use std::time::Duration;
use chrono::Local;
use serde_json::json;

use crate::models::{get_pay_type_name, PayRequest, PayResultData};
use crate::mqtt::MqttClient;

const MQTT_HOST: &str = "121.37.253.10:1883";
const MQTT_USER: &str = "dr-emqx";
const MQTT_PWD:  &str = "qNnbPZZ6yj4Ynyx5NoIg";

/// 执行支付注入的核心逻辑
pub fn execute_payment(req: &PayRequest) -> Result<PayResultData, String> {
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
        MQTT_HOST,
        &client_id,
        MQTT_USER,
        MQTT_PWD,
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

/// 联动触发目标车场的出场放行 HTTP 接口
fn trigger_auto_out(server_ip: &str, car_no: &str, dsn: &str) -> String {
    use std::io::Write;
    use std::net::TcpStream;

    let target_dsn = if dsn.is_empty() { "SIM21712648130131729" } else { dsn };
    let json_body = json!({
        "carNo": car_no,
        "controlIP": "192.168.151.47",
        "controlMac": target_dsn,
        "equipmentID": target_dsn,
        "inCarNo": car_no
    }).to_string();

    let host = format!("{}:8089", server_ip);
    let mut stream = match TcpStream::connect_timeout(&host.parse().unwrap_or(([192,168,65,58], 8089).into()), Duration::from_secs(3)) {
        Ok(s) => s,
        Err(e) => return format!("连接车场接口失败 (IP: {}): {}", server_ip, e),
    };

    let http_req = format!(
        "POST /box/handCarOut HTTP/1.1\r\n\
        Host: {}\r\n\
        Content-Type: application/json\r\n\
        Content-Length: {}\r\n\
        boxId: 1\r\n\
        userName: 001\r\n\
        Connection: close\r\n\r\n{}",
        host,
        json_body.len(),
        json_body
    );

    if let Err(e) = stream.write_all(http_req.as_bytes()) {
        return format!("发送出场请求失败: {}", e);
    }

    "出场放行指令已成功派发至目标车场".to_string()
}
