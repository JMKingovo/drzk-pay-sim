use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use chrono::Local;

use crate::models::{
    Booth, DetectedPopup, ParkDetails, ParkHttpInfo, ParkOverride, PayResultData, ServerConfig,
};
use crate::remote;

pub type SharedPopup = Arc<Mutex<Option<DetectedPopup>>>;
pub type SharedConfig = Arc<Mutex<ServerConfig>>;
/// 本进程内已下发的支付记录 (最近若干笔，供界面刷新后仍能查看)
pub type SharedPayHistory = Arc<Mutex<Vec<PayResultData>>>;

/// 已下发并持久化留存的支付记录最多保留条数 (长期保存所有测试流水)
pub const PAY_HISTORY_LIMIT: usize = 2000;

pub fn start_ws_listener(shared_config: SharedConfig, shared_popup: SharedPopup) {
    let cfg = shared_config.clone();
    let pop = shared_popup.clone();

    std::thread::spawn(move || {
        let mut last_ip = String::new();
        loop {
            let (target_ip, park_no, box_id, user) = {
                let c = cfg.lock().unwrap();
                (c.server_ip.clone(), c.park_no.clone(), c.box_id.clone(), c.login_name.clone())
            };

            if target_ip.is_empty() {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }

            // 车场信息尚未探测成功时不做无意义的通道连接，
            // 否则通道握手失败的状态会把"探测失败"的真实原因覆盖掉
            if park_no.is_empty() {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }

            // 没有岗亭信息就没有可连接的通道，如实说明而不是去握手一个空路径
            if box_id.is_empty() || user.is_empty() {
                let mut c = cfg.lock().unwrap();
                c.is_connected = false;
                c.status_text = format!(
                    "车场 {} 已探测 (编号 {})，但未取到岗亭信息，无法监听出口出车；可重新点击「连接车场」重试",
                    target_ip, park_no
                );
                drop(c);
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }

            if target_ip != last_ip {
                println!("🔄 检测到目标车场 IP 切换: {} -> {}", last_ip, target_ip);
                last_ip = target_ip.clone();
            }

            let addr = format!("{}:8089", target_ip);
            let path = format!("/box/boxControl/{}/{}", box_id, percent_encode(&user));

            match connect_ws(&addr, &path) {
                Ok(mut stream) => {
                    let connected_text = if park_no.is_empty() {
                        format!("已连通 {}:8089，但车场编号未探测成功，请重新连接车场", target_ip)
                    } else {
                        format!("已连接 {}:8089 (岗亭:{}, 车场:{})", target_ip, box_id, park_no)
                    };
                    {
                        let mut c = cfg.lock().unwrap();
                        c.is_connected = true;
                        c.status_text = connected_text;
                    }
                    println!("🟢 成功连通车场 [{}] WebSocket: ws://{}{}", target_ip, addr, path);
                    listen_frames(&mut stream, &cfg, &pop);
                    
                    {
                        let mut c = cfg.lock().unwrap();
                        c.is_connected = false;
                        c.status_text = format!("连接中断，正在重连 {}:8089...", target_ip);
                    }
                }
                Err(e) => {
                    {
                        let mut c = cfg.lock().unwrap();
                        c.is_connected = false;
                        c.status_text = format!("无法连接 {}:8089 ({})", target_ip, e);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// 主动探测车场配置 (从 MySQL 查出真实 PARK_NUM、在线岗亭及专属支付 MQTT 凭证)
///
/// 探测失败一律返回 Err，绝不回退到任何写死的车场编号或 MQTT 地址。
pub fn probe_server_details(server_ip: &str) -> Result<ParkDetails, String> {
    // 1. 查系统参数中的 PARK_NUM 和支付 MQTT Broker 配置
    let sql_params = "SELECT parameter_code, parameter_value FROM sys_parameters \
        WHERE parameter_code IN ('PARK_NUM', 'MQTT_SETTING_PAY_URL_ONLINE', 'MQTT_SETTING_PAY_USERNAME', 'MQTT_SETTING_PAY_PASSWORD');";

    let params_out = remote::run_sql(server_ip, sql_params)
        .map_err(|e| format!("读取车场参数失败: {}", e))?;

    let mut park_no = String::new();
    let mut mqtt_host = String::new();
    let mut mqtt_user = String::new();
    let mut mqtt_pwd = String::new();

    for row in remote::parse_rows(&params_out) {
        if row.len() < 2 {
            continue;
        }
        match row[0].as_str() {
            "PARK_NUM" => park_no = row[1].clone(),
            "MQTT_SETTING_PAY_URL_ONLINE" => {
                // 例如 "tcp://a.drzk.cn:62181,tcp://b.drzk.cn:62181" 或 "tcp://121.37.253.10:1883"
                let first_url = row[1].split(',').next().unwrap_or("").trim();
                let clean_host = first_url
                    .trim_start_matches("tcp://")
                    .trim_start_matches("ssl://")
                    .trim();
                if !clean_host.is_empty() {
                    mqtt_host = clean_host.to_string();
                }
            }
            "MQTT_SETTING_PAY_USERNAME" => mqtt_user = row[1].clone(),
            "MQTT_SETTING_PAY_PASSWORD" => mqtt_pwd = row[1].clone(),
            _ => {}
        }
    }

    if park_no.is_empty() {
        return Err(format!(
            "车场 {} 的数据库里没有 PARK_NUM，无法确定车场编号",
            server_ip
        ));
    }

    if mqtt_host.is_empty() {
        return Err(format!(
            "车场 {} 未配置 MQTT_SETTING_PAY_URL_ONLINE，无法确定支付 Broker 地址",
            server_ip
        ));
    }

    // 2. 查该车场的全部岗亭
    //    既用于连接出口监听通道，也存下来供界面下拉选择（下次没有 SSH 时也能选）。
    //    岗亭只影响出口监听与开闸联动，不影响支付投递，取不到时也不判定整体失败。
    let (booths, box_id, login_name) = match query_booths(server_ip) {
        Ok(booths) => match booths.first() {
            Some(b) => (booths.clone(), b.box_id.clone(), b.login_name.clone()),
            None => {
                println!("  ⚠️ 车场 {} 未取到岗亭信息，出口监听与开闸联动将不可用", server_ip);
                (Vec::new(), String::new(), String::new())
            }
        },
        Err(e) => {
            println!("  ⚠️ 车场 {} 的岗亭信息读取失败 ({}), 出口监听与开闸联动将不可用", server_ip, e);
            (Vec::new(), String::new(), String::new())
        }
    };

    Ok(ParkDetails {
        park_no,
        box_id,
        login_name,
        mqtt_host,
        mqtt_user,
        mqtt_pwd,
        booths,
    })
}

/// 查该车场的全部岗亭 (SSH+MySQL)
///
/// 实测岗亭 online 标志为 0 时通道依然接受连接，因此按"在线优先、机号最小"排序返回，
/// 由调用方取第一个作为连接目标，但不强制要求在线。
pub fn query_booths(server_ip: &str) -> Result<Vec<Booth>, String> {
    let sql =
        "SELECT box_id, loginName, online FROM park_local_set ORDER BY online DESC, box_id ASC;";
    let out = remote::run_sql(server_ip, sql)?;

    Ok(remote::parse_rows(&out)
        .into_iter()
        .filter(|r| r.len() >= 2 && !r[0].is_empty())
        .map(|r| Booth {
            box_id: r[0].clone(),
            login_name: r[1].clone(),
            online: r.get(2).map(|v| v == "1").unwrap_or(false),
        })
        .collect())
}

/// 应用"用户指定岗亭"的偏好
///
/// 指定了就优先用指定的（尊重用户选择）；指定的岗亭已不在车场岗亭列表中时回落到
/// 车场推荐的那个，并把说明文字返回给调用方展示。
pub fn apply_pinned_booth(values: &mut ParkDetails, manual: Option<&ParkOverride>) -> Option<String> {
    let m = manual.filter(|m| m.is_pinned())?;
    let pinned_box = m.box_id.clone().filter(|v| !v.trim().is_empty())?;
    let pinned_user = m.login_name.clone().unwrap_or_default();

    if values.booths.iter().any(|b| b.box_id == pinned_box) {
        let online = values
            .booths
            .iter()
            .find(|b| b.box_id == pinned_box)
            .map(|b| b.online)
            .unwrap_or(false);
        let auto_box = values.booths.first().map(|b| b.box_id.clone()).unwrap_or_default();
        values.box_id = pinned_box.clone();
        values.login_name = pinned_user;
        println!("  → 按界面指定的岗亭接入: {} / {}", values.box_id, values.login_name);

        if auto_box != pinned_box {
            return Some(format!(
                "已按指定使用 {}，车场在线的是 {}{}",
                pinned_box,
                auto_box,
                if online { "" } else { "（指定的岗亭当前离线）" }
            ));
        }
        None
    } else if values.booths.is_empty() {
        // 拿不到岗亭列表时仍按指定值用
        values.box_id = pinned_box.clone();
        values.login_name = pinned_user;
        println!("  → 未取到岗亭列表，仍按界面指定的岗亭 {} 接入", values.box_id);
        None
    } else {
        let note = format!(
            "指定的 {} 不在车场岗亭列表中，已改用 {}",
            pinned_box, values.box_id
        );
        Some(note)
    }
}

/// 岗亭自动跟随：车场哪个岗亭在线就连哪个
///
/// 车场侧的岗亭会切换（操作员换机、岗亭心跳转移），这里定期复查，
/// 发现"在线岗亭"与当前连接的不一致就切过去，避免一直连着一个已经没人的岗亭。
/// 用户在「车场参数」里指定了岗亭时不做自动切换（尊重指定）。
pub fn start_booth_watcher(cfg: SharedConfig, interval_secs: u64) {
    std::thread::spawn(move || {
        // 读取失败时不刷屏，但隔一段时间要报一次，否则现场无法判断"跟随"到底有没有在工作
        let mut fail_count: u32 = 0;
        loop {
        std::thread::sleep(Duration::from_secs(interval_secs));

        let (ip, park_no, cur_box, cur_user) = {
            let c = cfg.lock().unwrap();
            (
                c.server_ip.clone(),
                c.park_no.clone(),
                c.box_id.clone(),
                c.login_name.clone(),
            )
        };
        if ip.is_empty() || park_no.is_empty() {
            continue;
        }

        // 指定了岗亭就不自动切换
        if let Some(m) = remote::park_override(&ip) {
            if m.box_id.as_deref().map_or(false, |v| !v.trim().is_empty()) {
                continue;
            }
        }

        match query_booths(&ip) {
            Ok(booths) => {
                fail_count = 0;
                if let Some(best) = booths.first() {
                    if best.box_id != cur_box || best.login_name != cur_user {
                        println!(
                            "🔄 车场 [{}] 的在线岗亭已变为 {} / {}，切换通道连接（原 {} / {}）",
                            ip, best.box_id, best.login_name, cur_box, cur_user
                        );
                        let mut c = cfg.lock().unwrap();
                        c.box_id = best.box_id.clone();
                        c.login_name = best.login_name.clone();
                    }
                }
            }
            Err(e) => {
                // SSH 偶发失败很常见，不刷屏；但第一次和每隔 20 次报一条，便于现场判断
                fail_count += 1;
                if fail_count == 1 || fail_count % 20 == 0 {
                    println!(
                        "  ⚠️ 岗亭自动跟随暂时读不到车场岗亭列表 (已连续失败 {} 次): {}",
                        fail_count, e
                    );
                }
            }
        }
        }
    });
}

/// 通过 HTTP 直接向车场服务要车场信息 (POST /box/push/getParkInfo)
///
/// 实测该接口不校验身份，因此**不需要 SSH** 就能拿到车场编号，
/// 用于自动填充与核对（手填的编号若与它不一致，说明填错了，报文会投递到没人订阅的主题）。
pub fn fetch_park_info(server_ip: &str) -> Result<ParkHttpInfo, String> {
    let host = if server_ip.contains(':') {
        server_ip.to_string()
    } else {
        format!("{}:8089", server_ip)
    };
    let body = "{}";
    let request = format!(
        "POST /box/push/getParkInfo HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        host,
        body.len(),
        body
    );

    let mut stream = TcpStream::connect_timeout(
        &host.parse().map_err(|e| format!("车场地址解析失败: {}", e))?,
        Duration::from_secs(3),
    )
    .map_err(|e| format!("连接车场 {} 失败: {}", host, e))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
    stream.write_all(request.as_bytes()).map_err(|e| format!("发送请求失败: {}", e))?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| format!("读取响应失败: {}", e))?;
    let resp = String::from_utf8_lossy(&buf);

    let payload = match resp.find("\r\n\r\n") {
        Some(pos) => &resp[pos + 4..],
        None => return Err("车场响应格式异常".to_string()),
    };
    let (start, end) = match (payload.find('{'), payload.rfind('}')) {
        (Some(s), Some(e)) => (s, e),
        _ => return Err("车场未返回 JSON".to_string()),
    };

    let v: serde_json::Value =
        serde_json::from_str(&payload[start..=end]).map_err(|e| format!("解析车场信息失败: {}", e))?;
    let b = v.get("body").ok_or_else(|| "车场信息里没有 body".to_string())?;

    let park_no = b.get("projectNo").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if park_no.is_empty() {
        return Err("车场信息里没有 projectNo".to_string());
    }

    Ok(ParkHttpInfo {
        park_no,
        project_name: b.get("projectName").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        payment: b.get("payment").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    })
}

fn connect_ws(addr: &str, path: &str) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect_timeout(&addr.parse().map_err(|e| format!("{}", e))?, Duration::from_secs(3))
        .map_err(|e| format!("{}", e))?;
    stream.set_read_timeout(Some(Duration::from_secs(45))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let handshake = format!(
        "GET {} HTTP/1.1\r\n\
        Host: {}\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 13\r\n\r\n",
        path, addr
    );

    stream.write_all(handshake.as_bytes()).map_err(|e| format!("{}", e))?;

    let mut header_bytes = Vec::new();
    let mut single = [0u8; 1];
    while !header_bytes.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut single).is_err() {
            return Err("读取握手失败".into());
        }
        header_bytes.push(single[0]);
        if header_bytes.len() > 4096 {
            return Err("响应过长".into());
        }
    }

    let resp_str = String::from_utf8_lossy(&header_bytes);
    if resp_str.contains("101") {
        Ok(stream)
    } else {
        Err(format!("握手被拒绝: {}", resp_str.lines().next().unwrap_or("")))
    }
}

fn listen_frames(stream: &mut TcpStream, cfg: &SharedConfig, shared_popup: &SharedPopup) {
    let (current_ip, current_box, current_user) = {
        let c = cfg.lock().unwrap();
        (c.server_ip.clone(), c.box_id.clone(), c.login_name.clone())
    };

    loop {
        // 目标 IP 或岗亭发生变化时，立即断开当前 stream 退出重连
        {
            let c = cfg.lock().unwrap();
            if c.server_ip != current_ip {
                println!("🔌 目标 IP 已变更，断开旧连接 [{}]", current_ip);
                break;
            }
            if c.box_id != current_box || c.login_name != current_user {
                println!(
                    "🔌 岗亭已切换 ({} / {} → {} / {})，重连通道",
                    current_box, current_user, c.box_id, c.login_name
                );
                break;
            }
        }

        let mut header = [0u8; 2];
        if let Err(e) = stream.read_exact(&mut header) {
            if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
                let ping_frame = [0x89, 0x80, 0x11, 0x22, 0x33, 0x44];
                if stream.write_all(&ping_frame).is_err() {
                    break;
                }
                continue;
            }
            break;
        }

        let opcode = header[0] & 0x0F;
        if opcode == 0x8 {
            break;
        }

        let is_masked = (header[1] & 0x80) != 0;
        let mut payload_len = (header[1] & 0x7F) as usize;

        if payload_len == 126 {
            let mut ext = [0u8; 2];
            if stream.read_exact(&mut ext).is_err() { break; }
            payload_len = u16::from_be_bytes(ext) as usize;
        } else if payload_len == 127 {
            let mut ext = [0u8; 8];
            if stream.read_exact(&mut ext).is_err() { break; }
            payload_len = u64::from_be_bytes(ext) as usize;
        }

        let mut mask_key = [0u8; 4];
        if is_masked {
            if stream.read_exact(&mut mask_key).is_err() { break; }
        }

        let mut payload = vec![0u8; payload_len];
        if stream.read_exact(&mut payload).is_err() { break; }

        if is_masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask_key[i % 4];
            }
        }

        if opcode == 0x1 {
            if let Ok(text) = String::from_utf8(payload) {
                parse_and_update_popup(&text, shared_popup);
            }
        } else if opcode == 0x9 {
            let pong_frame = [0x8A, 0x80, 0xAA, 0xBB, 0xCC, 0xDD];
            let _ = stream.write_all(&pong_frame);
        }
    }
}

fn parse_and_update_popup(json_text: &str, shared_popup: &SharedPopup) {
    let v: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(val) => val,
        Err(_) => return,
    };

    let method = v.get("head").and_then(|h| h.get("method")).and_then(|m| m.as_str()).unwrap_or("");

    if method == "parkOutIsOpen" || method == "centerPayType" {
        let body = match v.get("body") {
            Some(b) => b,
            None => return,
        };

        let out_record = body.get("outRecord");
        let pay_vo = out_record.and_then(|o| o.get("payMentVo"));

        let car_no = pay_vo
            .and_then(|p| p.get("carNo"))
            .and_then(|c| c.as_str())
            .or_else(|| out_record.and_then(|o| o.get("carNo")).and_then(|c| c.as_str()))
            .or_else(|| body.get("carNo").and_then(|c| c.as_str()))
            .unwrap_or("")
            .to_string();

        if car_no.is_empty() {
            return;
        }

        let raw_fee = pay_vo
            .and_then(|p| p.get("payCharge"))
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);
        
        // 系统底层 payCharge 单位严格为【分】，统一除以 100.0 转换为【元】
        let money = raw_fee / 100.0;

        let dsn = body
            .get("controlMac")
            .and_then(|d| d.as_str())
            .or_else(|| out_record.and_then(|o| o.get("channelSet")).and_then(|cs| cs.get("dsn")).and_then(|d| d.as_str()))
            .unwrap_or("")
            .to_string();

        let channel_name = out_record
            .and_then(|o| o.get("outChannelName"))
            .and_then(|n| n.as_str())
            .or_else(|| out_record.and_then(|o| o.get("channelSet")).and_then(|cs| cs.get("channelName")).and_then(|n| n.as_str()))
            .unwrap_or("出口通道")
            .to_string();

        let in_time = pay_vo
            .and_then(|p| p.get("inTime"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        let popup = DetectedPopup {
            car_no: car_no.clone(),
            money,
            dsn,
            in_time,
            channel_name,
            popup_time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            method: method.to_string(),
        };

        println!(
            "🔔 【实时捕获收费弹窗】车牌: {}, 金额: {:.2} 元, 通道: {}",
            popup.car_no, popup.money, popup.channel_name
        );

        let mut lock = shared_popup.lock().unwrap();
        *lock = Some(popup);
    } else if method == "cancelCharge" {
        let mut lock = shared_popup.lock().unwrap();
        *lock = None;
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
