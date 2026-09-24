use std::time::Duration;
use chrono::Local;
use serde_json::json;

use crate::models::{get_pay_type_name, PayRequest, PayResultData};
use crate::mqtt::MqttClient;

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
    let entry_time = (now - chrono::Duration::minutes(30)).format("%Y-%m-%d %H:%M:%S").to_string();
    
    // 生成随机尾缀与唯一单号
    let random_str = format!("{:08x}", rand_u32());
    let order_num = format!("4500000499{}{}", now_ts, random_str);
    let out_trade_no = format!("SXF{}{}", now_ts, random_str);

    let in_id = if req.in_id.is_empty() {
        format!("in_guid_{}", &random_str)
    } else {
        req.in_id.clone()
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
            "channelType": 0,
            "remark": "Rust支付模拟器注入",
            "mch_id": "399251016876812",
            "parkId": "",
            "paymentType": 1,
            "duration": 30,
            "insidePay": 0.0,
            "inId": in_id,
            "couponAmount": req.money,
            "payType": req.pay_type,
            "carNo": req.car_no,
            "resourceRuleId": 0,
            "paymentScene": if req.dsn.is_empty() { "IN_LOT" } else { "OUT_LOT" },
            "freeTime": 0,
            "outTradeNo": out_trade_no,
            "inPic": "",
            "machNo": if req.dsn.is_empty() { 0 } else { 6 },
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
            "header": "TEMP",
            "channelName": if req.dsn.is_empty() { "" } else { "02出口" },
            "paymentTnx": order_num,
            "dsn": req.dsn,
            "isOnLineDis": 0
        }
    });

    let message = pay_body.to_string();

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
fn trigger_auto_out(server_ip: &str, car_no: &str, dsn: &str) -> String {
    use std::io::Write;
    use std::net::TcpStream;
    use std::process::Command;

    // 1. 动态探测该车场的在线岗亭、登录人和通道IP
    let mut box_id = "1".to_string();
    let mut user = "超级管理员".to_string();
    let mut channel_ip = "192.168.151.47".to_string();
    let mut actual_dsn = dsn.to_string();

    let sql = "PAGER=cat mysql -uroot -p123456 ykt -B -e \"\
        SELECT l.box_id, l.loginName, c.channel_ip, c.dsn \
        FROM park_local_set l \
        LEFT JOIN park_channel_set c ON l.box_id = c.box_id \
        WHERE l.online=1 AND c.in_out=1 LIMIT 1;\" 2>/dev/null";
    
    if let Ok(out) = Command::new("sshpass")
        .args(&["-p", "root", "ssh", "-o", "StrictHostKeyChecking=no", "-o", "ConnectTimeout=2", &format!("root@{}", server_ip), sql])
        .output()
    {
        let txt = String::from_utf8_lossy(&out.stdout);
        if let Some(val) = txt.lines().nth(1) {
            let parts: Vec<&str> = val.split('\t').collect();
            if parts.len() >= 4 {
                box_id = parts[0].trim().to_string();
                user = parts[1].trim().to_string();
                channel_ip = parts[2].trim().to_string();
                if actual_dsn.is_empty() {
                    actual_dsn = parts[3].trim().to_string();
                }
            }
        }
    }

    if actual_dsn.is_empty() {
        actual_dsn = if server_ip.ends_with(".59") {
            "2211169205100691266100149727978060dcdbdeb56de0ac99d6bb9a3fe718e2".to_string()
        } else {
            "SIM21712648130131729".to_string()
        };
    }

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

    let mut stream = match TcpStream::connect_timeout(&host.parse().unwrap_or(([192,168,65,59], 8089).into()), Duration::from_secs(3)) {
        Ok(s) => s,
        Err(e) => return format!("连接车场接口失败 (IP: {}): {}", server_ip, e),
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
