use tiny_http::{Header, Response, Server};
use std::time::Duration;
use std::net::TcpStream;
use std::io::{Read, Write};
use chrono::Local;

use crate::models::{
    ApiResponse, DetectedPopup, PayRequest, RemoteConfigView, RemoteCredentials, SwitchServerRequest,
};
use crate::remote;
use crate::service::execute_payment;
use crate::ws_listener::{probe_server_details, SharedConfig, SharedPopup};

/// 组装回给界面的凭据视图 (不含密码明文)
fn remote_config_view() -> RemoteConfigView {
    let c = remote::current();
    RemoteConfigView {
        ssh_user: c.ssh_user.clone(),
        ssh_port: c.ssh_port.clone(),
        db_user: c.db_user.clone(),
        db_name: c.db_name.clone(),
        ssh_pwd_set: !c.ssh_pwd.is_empty(),
        db_pwd_set: !c.db_pwd.is_empty(),
        config_file: remote::config_path().display().to_string(),
    }
}

pub fn start_http_server(addr: &str, shared_config: SharedConfig, shared_popup: SharedPopup) {
    let server = match Server::http(addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("【错误】HTTP 服务启动失败 (绑定 {}): {}", addr, e);
            return;
        }
    };

    println!("============================================================");
    println!("  🚀 道尔车场支付模拟器 - HTTP 服务已启动！");
    println!("  📡 监听地址: http://{}", addr);
    println!("  📌 支付接口: POST http://{}/api/pay", addr);
    println!("  🔌 切换车场: POST http://{}/api/server/switch", addr);
    println!("  🔔 弹窗检测: GET  http://{}/api/popup/current", addr);
    println!("  🔍 状态检查: GET  http://{}/api/status", addr);
    println!("============================================================");

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let method = request.method().as_str().to_uppercase();

        // 统一处理 OPTIONS 跨域预检
        if method == "OPTIONS" {
            let resp = Response::empty(200)
                .with_header(Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                .with_header(Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"POST, GET, OPTIONS"[..]).unwrap())
                .with_header(Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type"[..]).unwrap());
            request.respond(resp).ok();
            continue;
        }

        // 1. 切换目标车场 IP (POST /api/server/switch)
        if url == "/api/server/switch" && method == "POST" {
            let mut body_str = String::new();
            if request.as_reader().read_to_string(&mut body_str).is_err() {
                send_json(request, 400, ApiResponse::<()> { code: 400, msg: "读取请求失败".into(), data: None });
                continue;
            }

            let switch_req: SwitchServerRequest = match serde_json::from_str(&body_str) {
                Ok(r) => r,
                Err(e) => {
                    send_json(request, 400, ApiResponse::<()> { code: 400, msg: format!("JSON 解析失败: {}", e), data: None });
                    continue;
                }
            };

            let new_ip = switch_req.server_ip.trim();
            if new_ip.is_empty() {
                send_json(request, 400, ApiResponse::<()> { code: 400, msg: "车场 IP 不能为空".into(), data: None });
                continue;
            }

            println!("🔄 正在连接并探测目标车场 [{}]...", new_ip);
            let probed = match probe_server_details(new_ip) {
                Ok(d) => d,
                Err(e) => {
                    // 探测失败即拒绝切换，保持原车场配置不变，避免带着未知编号去下发支付
                    eprintln!("  -> 车场探测失败: {}", e);
                    send_json(request, 500, ApiResponse::<()> {
                        code: 500,
                        msg: format!(
                            "无法探测车场 {} 的真实信息: {}\n如该车场凭据不同，请在界面顶部「SSH / 数据库」中修改后重新连接。",
                            new_ip, e
                        ),
                        data: None,
                    });
                    continue;
                }
            };
            let park_no = switch_req.park_no.unwrap_or_else(|| probed.park_no.clone());

            // 更新全局配置
            let new_cfg = {
                let mut c = shared_config.lock().unwrap();
                // 目标 IP 未变时沿用原有通道连接状态，避免把已连通的通道误标为断开
                let same_ip = c.server_ip == new_ip;
                let still_connected = same_ip && c.is_connected;
                c.server_ip = new_ip.to_string();
                c.park_no = park_no.clone();
                c.box_id = probed.box_id.clone();
                c.login_name = probed.login_name.clone();
                c.is_connected = still_connected;
                c.status_text = if still_connected {
                    format!("已连接 {}:8089 (车场: {})", new_ip, park_no)
                } else {
                    format!("已探测 {}:8089 (车场: {})，正在连接通道...", new_ip, park_no)
                };
                c.mqtt_host = probed.mqtt_host.clone();
                c.mqtt_user = probed.mqtt_user.clone();
                c.mqtt_pwd = probed.mqtt_pwd.clone();
                c.clone()
            };

            // 清空旧车场的弹窗缓存
            {
                let mut p = shared_popup.lock().unwrap();
                *p = None;
            }

            println!("  -> 车场编号探测: {}, 在线岗亭: {}, 操作员: {}", park_no, probed.box_id, probed.login_name);
            send_json(request, 200, ApiResponse {
                code: 200,
                msg: format!("已成功切换至目标车场 {}", new_ip),
                data: Some(new_cfg),
            });
            continue;
        }

        // 2. 获取当前车场配置 (GET /api/server/config)
        if url == "/api/server/config" && method == "GET" {
            let cfg = { shared_config.lock().unwrap().clone() };
            send_json(request, 200, ApiResponse { code: 200, msg: "OK".into(), data: Some(cfg) });
            continue;
        }

        // 2.1 读取当前 SSH / 数据库凭据 (密码只回显是否已设置)
        if url == "/api/remote/config" && method == "GET" {
            send_json(request, 200, ApiResponse { code: 200, msg: "OK".into(), data: Some(remote_config_view()) });
            continue;
        }

        // 2.2 修改 SSH / 数据库凭据并落盘保存
        if url == "/api/remote/config" && method == "POST" {
            let mut body_str = String::new();
            if request.as_reader().read_to_string(&mut body_str).is_err() {
                send_json(request, 400, ApiResponse::<()> { code: 400, msg: "读取请求失败".into(), data: None });
                continue;
            }

            let creds: RemoteCredentials = match serde_json::from_str(&body_str) {
                Ok(c) => c,
                Err(e) => {
                    send_json(request, 400, ApiResponse::<()> { code: 400, msg: format!("JSON 解析失败: {}", e), data: None });
                    continue;
                }
            };

            let mut new_cfg = remote::current();
            new_cfg.apply_update(&creds);

            if let Err(e) = remote::save_file(&new_cfg) {
                send_json(request, 500, ApiResponse::<()> { code: 500, msg: format!("保存凭据失败: {}", e), data: None });
                continue;
            }

            println!(
                "🔧 已更新远程探测凭据: SSH {}@:{} / MySQL {}@{} (密码{}回显)",
                new_cfg.ssh_user, new_cfg.ssh_port, new_cfg.db_user, new_cfg.db_name,
                if creds.ssh_pwd.is_some() || creds.db_pwd.is_some() { "已" } else { "未" }
            );
            remote::set(new_cfg);

            send_json(request, 200, ApiResponse {
                code: 200,
                msg: "凭据已保存，请重新连接车场以生效".into(),
                data: Some(remote_config_view()),
            });
            continue;
        }

        // 3. 执行支付注入 (POST /api/pay)
        if (url == "/api/pay" || url == "/api/pay/simulate") && method == "POST" {
            let mut body_str = String::new();
            if let Err(e) = request.as_reader().read_to_string(&mut body_str) {
                send_json(request, 400, ApiResponse::<()> { code: 400, msg: format!("读取请求体失败: {}", e), data: None });
                continue;
            }

            let mut pay_req: PayRequest = match serde_json::from_str(&body_str) {
                Ok(r) => r,
                Err(e) => {
                    send_json(request, 400, ApiResponse::<()> { code: 400, msg: format!("JSON 请求解析失败: {}", e), data: None });
                    continue;
                }
            };

            // 请求未指定车场时，采用当前已探测成功的车场；两者皆无则拒绝下发
            {
                let c = shared_config.lock().unwrap();
                if pay_req.park_no.trim().is_empty() {
                    pay_req.park_no = c.park_no.clone();
                }
                if pay_req.server_ip.trim().is_empty() {
                    pay_req.server_ip = c.server_ip.clone();
                }
            }

            if pay_req.park_no.trim().is_empty() {
                send_json(request, 400, ApiResponse::<()> {
                    code: 400,
                    msg: "当前没有已探测成功的车场编号，无法确定支付投递目标。请先在顶部输入车场 IP 并点击「连接车场」。".into(),
                    data: None,
                });
                continue;
            }

            let (mqtt_host, mqtt_user, mqtt_pwd) = {
                let c = shared_config.lock().unwrap();
                (c.mqtt_host.clone(), c.mqtt_user.clone(), c.mqtt_pwd.clone())
            };

            if mqtt_host.trim().is_empty() {
                send_json(request, 400, ApiResponse::<()> {
                    code: 400,
                    msg: format!("未获取到车场 {} 的支付 MQTT 配置，无法下发。请重新连接车场。", pay_req.park_no),
                    data: None,
                });
                continue;
            }

            println!(
                "[下发支付] 车牌: {}, 金额: {}元, 目标车场IP: {}, 车场编号: {}, 方式: {}, MQTT Broker: {}",
                pay_req.car_no, pay_req.money, pay_req.server_ip, pay_req.park_no, pay_req.pay_type, mqtt_host
            );

            match execute_payment(&pay_req, &mqtt_host, &mqtt_user, &mqtt_pwd) {
                Ok(result) => {
                    println!("  -> 注入成功！单号: {}", result.order_num);
                    let mut lock = shared_popup.lock().unwrap();
                    *lock = None;

                    send_json(request, 200, ApiResponse {
                        code: 200,
                        msg: "支付结果已成功注入到目标车场".to_string(),
                        data: Some(result),
                    });
                }
                Err(err) => {
                    eprintln!("  -> 注入失败: {}", err);
                    send_json(request, 500, ApiResponse::<()> {
                        code: 500,
                        msg: format!("支付下发失败: {}", err),
                        data: None,
                    });
                }
            }
            continue;
        }

        // 4. 当前弹窗轮询 (GET /api/popup/current)
        if url.starts_with("/api/popup/current") && method == "GET" {
            let mut current = {
                let lock = shared_popup.lock().unwrap();
                lock.clone()
            };

            if current.is_none() {
                let server_ip = { shared_config.lock().unwrap().server_ip.clone() };
                // 尚未选定车场时不必去抓日志，避免无意义的 SSH 尝试
                if !server_ip.trim().is_empty() {
                    if let Some(sniffed) = sniff_latest_popup(&server_ip) {
                        let mut lock = shared_popup.lock().unwrap();
                        *lock = Some(sniffed.clone());
                        current = Some(sniffed);
                    }
                }
            }

            send_json(request, 200, ApiResponse { code: 200, msg: "OK".to_string(), data: current });
            continue;
        }

        // 5. 清除弹窗缓存
        if url == "/api/popup/clear" && method == "POST" {
            let mut lock = shared_popup.lock().unwrap();
            *lock = None;
            send_json(request, 200, ApiResponse::<()> { code: 200, msg: "已清除当前弹窗缓存".to_string(), data: None });
            continue;
        }

        // 6. 查算费
        if url.starts_with("/api/fee/current") && method == "GET" {
            handle_query_fee(request, &url, &shared_config);
            continue;
        }

        // 7. 静态页面
        if url == "/" || url == "/index.html" {
            let html_content = include_str!("index.html");
            let resp = Response::from_string(html_content)
                .with_status_code(200)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap())
                .with_header(Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap());
            request.respond(resp).ok();
            continue;
        }

        if url == "/api/status" || url == "/api/health" {
            let cfg = { shared_config.lock().unwrap().clone() };
            let status_json = serde_json::json!({
                "service": "drzk-pay-sim",
                "status": "RUNNING",
                "version": "0.5.0",
                "current_server": cfg
            });
            send_raw_json(request, 200, status_json.to_string());
            continue;
        }

        send_raw_json(request, 404, r#"{"code":404,"msg":"未找到对应路由接口"}"#.to_string());
    }
}

/// 主动调用车场后台算费接口 GET /box/fee/current?carNo=...
fn handle_query_fee(request: tiny_http::Request, url_str: &str, shared_config: &SharedConfig) {
    let default_ip = { shared_config.lock().unwrap().server_ip.clone() };
    let mut server_ip = default_ip;
    let mut car_no = "".to_string();

    if let Some(pos) = url_str.find('?') {
        let query = &url_str[pos + 1..];
        for pair in query.split('&') {
            let mut kv = pair.splitn(2, '=');
            let k = kv.next().unwrap_or("");
            let v = kv.next().unwrap_or("");
            if k == "server_ip" && !v.is_empty() {
                server_ip = v.to_string();
            } else if k == "car_no" {
                car_no = percent_decode(v);
            }
        }
    }

    if car_no.is_empty() {
        send_json(request, 400, ApiResponse::<()> { code: 400, msg: "缺少 car_no 参数".to_string(), data: None });
        return;
    }

    let fee_url = format!("http://{}:8089/box/fee/current?carNo={}", server_ip, percent_encode(&car_no));
    match fetch_http_json(&fee_url) {
        Ok(json_val) => {
            let status = json_val.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
            if status == 1 {
                let data = json_val.get("data");
                let pay_charge_cents = data.and_then(|d| d.get("payCharge")).and_then(|c| c.as_f64()).unwrap_or(0.0);
                let money = pay_charge_cents / 100.0;
                let in_time = data.and_then(|d| d.get("inTime")).and_then(|t| t.as_str()).unwrap_or("");
                let stay_min = data.and_then(|d| d.get("stayMinute")).and_then(|m| m.as_i64()).unwrap_or(0);

                let res_data = serde_json::json!({
                    "car_no": car_no,
                    "money": money,
                    "in_time": in_time,
                    "stay_minute": stay_min
                });

                send_raw_json(request, 200, serde_json::json!({ "code": 200, "msg": "获取成功", "data": res_data }).to_string());
            } else {
                let msg = json_val.get("msg").and_then(|m| m.as_str()).unwrap_or("车辆无欠费或不在场内");
                send_raw_json(request, 200, serde_json::json!({ "code": 400, "msg": msg }).to_string());
            }
        }
        Err(e) => {
            send_raw_json(request, 500, serde_json::json!({ "code": 500, "msg": format!("调用车场查费失败: {}", e) }).to_string());
        }
    }
}

fn sniff_latest_popup(server_ip: &str) -> Option<DetectedPopup> {
    // 1. 从目标车场的当前活跃日志中抓取当前正在通道停泊待收费的车辆与通道 DSN
    let cmd = "grep '当前车辆：' /wasHome/server/logs/web-box.log | tail -n 1 2>/dev/null";
    let line = remote::run_command(server_ip, cmd).ok()?;

    if !line.contains("当前车辆：") {
        return None;
    }

    // 解析格式: ... 通道<dsn>当前车辆：<car_no>
    let pos_car = line.find("当前车辆：")?;
    let car_no = line[pos_car + "当前车辆：".len()..].trim();
    if car_no.is_empty() {
        return None;
    }

    let dsn = if let Some(pos_chan) = line.find("通道") {
        let after_chan = &line[pos_chan + "通道".len()..pos_car];
        after_chan.trim().to_string()
    } else {
        String::new()
    };

    // 2. 调用目标车场的 /box/fee/current 接口查询当前车辆精确计费
    let fee_url = format!("http://{}:8089/box/fee/current?carNo={}", server_ip, percent_encode(car_no));
    let json_val = fetch_http_json(&fee_url).ok()?;
    let status = json_val.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
    if status != 1 {
        return None;
    }

    let data = json_val.get("data")?;
    let raw_fee = data.get("payCharge").and_then(|c| c.as_f64()).unwrap_or(0.0);
    // 系统底层 payCharge 单位严格为【分】，统一除以 100.0 转换为【元】
    let money = raw_fee / 100.0;

    let in_time = data.get("inTime").and_then(|t| t.as_str()).unwrap_or("").to_string();

    Some(DetectedPopup {
        car_no: car_no.to_string(),
        money,
        dsn: if dsn.is_empty() { "2211169205100691266100149727978060dcdbdeb56de0ac99d6bb9a3fe718e2".to_string() } else { dsn },
        in_time,
        channel_name: "出口".to_string(),
        popup_time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        method: "parkOutIsOpen".to_string(),
    })
}

fn fetch_http_json(url: &str) -> Result<serde_json::Value, String> {
    let stripped = url.trim_start_matches("http://");
    let mut parts = stripped.splitn(2, '/');
    let host_port = parts.next().unwrap_or("");
    let path = format!("/{}", parts.next().unwrap_or(""));

    let mut stream = TcpStream::connect_timeout(&host_port.parse().map_err(|e| format!("{}", e))?, Duration::from_secs(3))
        .map_err(|e| format!("{}", e))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();

    let req = format!("GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", path, host_port);
    stream.write_all(req.as_bytes()).map_err(|e| format!("{}", e))?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| format!("{}", e))?;
    let resp = String::from_utf8_lossy(&buf);

    if let Some(pos) = resp.find("\r\n\r\n") {
        let body = &resp[pos + 4..];
        if let (Some(start), Some(end)) = (body.find('{'), body.rfind('}')) {
            let json_str = &body[start..=end];
            serde_json::from_str(json_str).map_err(|e| format!("JSON 解析失败: {}", e))
        } else {
            serde_json::from_str(body.trim()).map_err(|e| format!("JSON 解析失败: {}", e))
        }
    } else {
        Err("HTTP 响应格式异常".to_string())
    }
}

fn percent_encode(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{:02X}", b));
        }
    }
    encoded
}

fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next().unwrap_or('0');
            let h2 = chars.next().unwrap_or('0');
            if let Ok(b) = u8::from_str_radix(&format!("{}{}", h1, h2), 16) {
                bytes.push(b);
            }
        } else if c == '+' {
            bytes.push(b' ');
        } else {
            bytes.push(c as u8);
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn send_json<T: serde::Serialize>(request: tiny_http::Request, status: u16, data: ApiResponse<T>) {
    let json_text = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
    send_raw_json(request, status, json_text);
}

fn send_raw_json(request: tiny_http::Request, status: u16, text: String) {
    let resp = Response::from_string(text)
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap());
    request.respond(resp).ok();
}
