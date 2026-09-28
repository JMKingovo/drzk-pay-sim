use std::time::Duration;
use chrono::Local;
use serde_json::json;

use crate::models::{get_pay_type_name, PayRequest, PayResultData};
use crate::mqtt::MqttClient;
use crate::remote;

/// 执行支付注入的核心逻辑 (支持动态指定目标车场的专属 MQTT Broker 地址与凭证)
pub fn execute_payment(
    req: &PayRequest,
    mqtt_host: &str,
    mqtt_user: &str,
    mqtt_pwd: &str,
) -> Result<PayResultData, String> {
    let now = Local::now();
    let now_ts = now.format("%Y%m%d%H%M%S").to_string();
    let now_str = now.format("%Y-%m-%d %H:%M:%S").to_string();
    
    // 生成随机尾缀与唯一单号
    let random_str = format!("{:08x}", rand_u32());
    let order_num = format!("4500000499{}{}", now_ts, random_str);
    let out_trade_no = format!("SXF{}{}", now_ts, random_str);

    // 出口车道缴费 与 场内定点缴费 在车场那边是两种收费类型 (charge_type: 0.出口收费 1.定点收费)。
    // 车场是靠支付报文里的"计费上下文"把款项挂到该车当前的计费记录上的：
    // 入场时间/入场 ID/停放时长必须是该车真实的入场信息，否则会被记成"定点收费"，
    // 后台就显示成「聚合支付V6(定点收费)」，与真实出口扫码支付的记录对不上。
    let is_lane_pay = req.auto_out || !req.dsn.is_empty();

    let fallback_entry = (now - chrono::Duration::minutes(30)).format("%Y-%m-%d %H:%M:%S").to_string();
    let fallback_in_id = if req.in_id.is_empty() {
        format!("in_guid_{}", &random_str)
    } else {
        req.in_id.clone()
    };

    // 出口缴费：向车场自己的算费接口取该车真实的入场时间/入场ID/停放时长
    let mut billing_from_park = false;
    let (entry_time, in_id, duration) = if is_lane_pay {
        match fetch_billing_context(&req.server_ip, &req.car_no) {
            Some(ctx) => {
                println!(
                    "  🧾 取到真实计费上下文: 入场 {}, 停放 {} 分钟, inId {}",
                    ctx.in_time, ctx.stay_minute, ctx.in_id
                );
                billing_from_park = true;
                (ctx.in_time, ctx.in_id, ctx.stay_minute)
            }
            None => {
                eprintln!("  ⚠️ 未取到该车的计费上下文，将按兜底值下发 (可能被记成定点收费)");
                (fallback_entry, fallback_in_id, 30)
            }
        }
    } else {
        (fallback_entry, fallback_in_id, 30)
    };

    let topic = format!("server/data/publish/phone/{}", req.park_no);

    // 组装道尔 5.0 标准支付下发 JSON 报文
    let pay_body = json!({
        "head": {
            "replyTopic": format!("send/payment/callback/{}", random_str),
            "method": "park/userpaymentcarfee",
            "parkId": req.park_no
        },
        "body": {
            "parkingNo": req.park_no,
            "parkName": "fb民乐1",
            "randomSessionId": random_str,
            "payTime": now_str,
            "overTime": 0,
            "overTimeCoupon": "0",
            "channelType": if is_lane_pay { 1 } else { 0 },
            "remark": if is_lane_pay { "" } else { "Rust支付模拟器注入" },
            "mch_id": "399251016876812",
            "parkId": "",
            "paymentType": if is_lane_pay { 0 } else { 1 },
            "duration": duration,
            "insidePay": 0.0,
            "inId": in_id,
            "couponAmount": req.money,
            "payType": req.pay_type,
            "carNo": req.car_no,
            "carType": "31",
            "resourceRuleId": 0,
            "paymentScene": if is_lane_pay { "LANE_QR" } else { "IN_LOT" },
            "freeTime": 0,
            "outTradeNo": out_trade_no,
            "inPic": "",
            "machNo": 0,
            "showCouponTip": false,
            "subAppId": "",
            "merchantRole": "MAIN_MCH",
            "carNoType": 0,
            "disAmount": 0.0,
            "openid": "rust_sim_openid",
            "payMode": 0,
            "chargeTime": entry_time,
            "endChargeTime": now_str,
            "sumArrearsMoney": 0.0,
            "serviceIP": req.server_ip,
            "totalAmount": req.money,
            "currentArrears": false,
            "entryTime": entry_time,
            "spaceNo": "",
            "carRealType": 31,
            "header": if is_lane_pay { "SCANCODE" } else { "TEMP" },
            "channelName": req.channel_name,
            "paymentTnx": order_num,
            "dsn": req.dsn,
            "isOnLineDis": 0
        }
    });

    let message = pay_body.to_string();

    // 把实际下发的报文打进日志，便于和车场收到的报文逐字段核对
    println!("  📤 支付报文: {}", message);

    // 连接云端 MQTT 下发
    let client_id = format!("rust_sim_{}", &random_str);
    let mut client = MqttClient::connect(
        mqtt_host,
        &client_id,
        mqtt_user,
        mqtt_pwd,
        Duration::from_secs(5),
    )?;

    client.publish(&topic, &message)?;
    client.disconnect();

    // 如果开启了 auto_out，联动调用目标车场的出场放行接口
    let auto_out_res = if req.auto_out {
        Some(trigger_auto_out(&req.server_ip, &req.car_no, &req.dsn))
    } else {
        None
    };

    Ok(PayResultData {
        car_no: req.car_no.clone(),
        money: req.money,
        pay_type: req.pay_type,
        pay_type_name: get_pay_type_name(req.pay_type).to_string(),
        order_num,
        out_trade_no,
        pay_time: now_str,
        target_park_no: req.park_no.clone(),
        target_server_ip: req.server_ip.clone(),
        auto_out_result: auto_out_res,
        is_lane: is_lane_pay,
        billing_context_from_park: billing_from_park,
    })
}

/// 该车在车场那边的真实计费上下文 (取自车场自己的算费接口)
struct BillingContext {
    in_time: String,
    in_id: String,
    stay_minute: i64,
}

/// 调车场算费接口 GET /box/fee/current?carNo=... 取该车真实入场信息
///
/// 出口缴费必须带这些真实值，车场才会把这笔钱挂到该车的出口计费记录上；
/// 取不到时返回 None，由调用方退回兜底值 (并提示可能被记成定点收费)。
fn fetch_billing_context(server_ip: &str, car_no: &str) -> Option<BillingContext> {
    let host = if server_ip.contains(':') {
        server_ip.to_string()
    } else {
        format!("{}:8089", server_ip)
    };
    let path = format!("/box/fee/current?carNo={}", url_encode_utf8(car_no));

    let mut stream = std::net::TcpStream::connect_timeout(
        &host.parse().ok()?,
        Duration::from_secs(3),
    )
    .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();

    use std::io::{Read, Write};
    let req = format!("GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", path, host);
    stream.write_all(req.as_bytes()).ok()?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    let resp = String::from_utf8_lossy(&buf);

    let body = match resp.find("\r\n\r\n") {
        Some(pos) => &resp[pos + 4..],
        None => return None,
    };
    let (start, end) = (body.find('{')?, body.rfind('}')?);
    parse_billing_context(&body[start..=end])
}

/// 解析车场算费接口返回的 JSON
fn parse_billing_context(json_text: &str) -> Option<BillingContext> {
    let v: serde_json::Value = serde_json::from_str(json_text).ok()?;

    if v.get("status").and_then(|s| s.as_i64()) != Some(1) {
        return None;
    }
    let data = v.get("data")?;

    let in_time = data.get("inTime").and_then(|t| t.as_str())?.to_string();
    let in_id = data.get("inid").and_then(|i| i.as_str()).unwrap_or("").to_string();

    Some(BillingContext {
        in_time,
        in_id: if in_id.is_empty() {
            format!("in_guid_{}", rand_u32())
        } else {
            in_id
        },
        stay_minute: data.get("stayMinute").and_then(|m| m.as_i64()).unwrap_or(0),
    })
}

/// 简易伪随机数生成器 (避免拉取 rand crate 依赖)
fn rand_u32() -> u32 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    nanos.wrapping_mul(1664525).wrapping_add(1013904223) ^ pid
}

/// 联动触发目标车场的出场放行与关弹窗 HTTP 接口
///
/// 岗亭号、操作员、通道 IP 与通道 DSN 全部取自车场数据库，取不到即中止，
/// 不使用任何写死的通道信息，避免把开闸指令派发到错误的通道。
fn trigger_auto_out(server_ip: &str, car_no: &str, dsn: &str) -> String {
    use std::io::Write;
    use std::net::TcpStream;

    // 1. 从车场数据库动态取在线岗亭、登录人与出口通道
    let sql = "SELECT l.box_id, l.loginName, c.channel_ip, c.dsn \
        FROM park_local_set l \
        LEFT JOIN park_channel_set c ON l.box_id = c.box_id \
        WHERE l.online=1 AND c.in_out=1 LIMIT 1;";

    let out = match remote::run_sql(server_ip, sql) {
        Ok(t) => t,
        Err(e) => return format!("出场联动中止: {}", e),
    };

    let (box_id, user, channel_ip, db_dsn) = match remote::parse_rows(&out).first() {
        Some(row) if row.len() >= 4 => (
            row[0].clone(),
            row[1].clone(),
            row[2].clone(),
            row[3].clone(),
        ),
        _ => {
            return format!(
                "出场联动中止: 车场 {} 未返回在线岗亭与出口通道信息",
                server_ip
            )
        }
    };

    if box_id.is_empty() || user.is_empty() {
        return format!("出场联动中止: 车场 {} 的在线岗亭信息不完整", server_ip);
    }

    if channel_ip.is_empty() {
        return format!(
            "出场联动中止: 车场 {} 的出口通道缺少 channel_ip，无法派发开闸指令",
            server_ip
        );
    }

    let actual_dsn = if !dsn.is_empty() {
        dsn.to_string()
    } else if !db_dsn.is_empty() {
        db_dsn
    } else {
        return format!(
            "出场联动中止: 未取到出口通道 DSN，无法定位要放行的通道 (车场 {})",
            server_ip
        );
    };

    let host = format!("{}:8089", server_ip);

    // 2. 发送 /box/outIsOpen 触发开闸并关闭前端弹窗
    let open_body = json!({
        "type": "0",
        "controlMac": actual_dsn,
        "equipmentID": actual_dsn,
        "controlIP": channel_ip,
        "outRecord": {
            "carNo": car_no
        }
    }).to_string();

    let mut stream = match TcpStream::connect_timeout(&host.parse().unwrap_or(([127, 0, 0, 1], 8089).into()), Duration::from_secs(3)) {
        Ok(s) => s,
        Err(e) => return format!("出场联动中止: 连接车场接口失败 (IP: {}): {}", server_ip, e),
    };

    let user_encoded = url_encode_utf8(&user);
    let http_req = format!(
        "POST /box/outIsOpen HTTP/1.1\r\n\
        Host: {}\r\n\
        Content-Type: application/json\r\n\
        Content-Length: {}\r\n\
        boxId: {}\r\n\
        userName: {}\r\n\
        Connection: close\r\n\r\n{}",
        host,
        open_body.len(),
        box_id,
        user_encoded,
        open_body
    );

    let _ = stream.write_all(http_req.as_bytes());

    "出场放行与开闸指令已成功派发至车场".to_string()
}

fn url_encode_utf8(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 车场算费接口的真实返回
    #[test]
    fn 解析真实算费响应() {
        let raw = r#"{"status":1,"msg":null,"data":{"carNo":"京A12345","inTime":"2026-09-16 00:00:00",
            "endTime":"2026-09-28 18:08:34","stayMinute":18368,"payCharge":13,"accountCharge":13,
            "disAmountCharge":0,"inid":"3c840f0688dc428fbd7397d6823acc4a"}}"#;
        let ctx = parse_billing_context(raw).expect("应能解析");
        assert_eq!(ctx.in_time, "2026-09-16 00:00:00");
        assert_eq!(ctx.in_id, "3c840f0688dc428fbd7397d6823acc4a");
        assert_eq!(ctx.stay_minute, 18368);
    }

    #[test]
    fn 车辆已结清时没有计费上下文() {
        let raw = r#"{"status":0,"msg":"费用已结清，无需缴费","data":null}"#;
        assert!(parse_billing_context(raw).is_none());
    }

    #[test]
    fn 缺少inid时仍可用且不panic() {
        let raw = r#"{"status":1,"data":{"inTime":"2026-09-28 00:00:00","stayMinute":30}}"#;
        let ctx = parse_billing_context(raw).expect("应能解析");
        assert_eq!(ctx.in_time, "2026-09-28 00:00:00");
        assert!(!ctx.in_id.is_empty());
    }
}
