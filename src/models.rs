use serde::{Deserialize, Serialize};

/// HTTP API 请求体 (支持指定任意车场 IP 和参数)
#[derive(Debug, Clone, Deserialize)]
pub struct PayRequest {
    /// 目标车牌号 (必填，例如: "粤B88801")
    pub car_no: String,

    /// 支付金额 (元)，默认 10.0
    #[serde(default = "default_money")]
    pub money: f64,

    /// 支付方式编码: 57=聚合支付V6, 1=微信, 2=支付宝, 0=现金, 21=ETC，默认 57
    #[serde(default = "default_pay_type")]
    pub pay_type: i32,

    /// 车场编号 (默认 "H51810900057")
    #[serde(default = "default_park_no")]
    pub park_no: String,

    /// 目标车场服务器 IP (例如: "192.168.65.58")
    #[serde(default = "default_server_ip")]
    pub server_ip: String,

    /// 通道 DSN (出口扫码必填；场内扫码留空即可)
    #[serde(default)]
    pub dsn: String,

    /// 车辆入场 ID (如果已知；留空则自动生成)
    #[serde(default)]
    pub in_id: String,

    /// 是否联动触发出口手工放行 (默认 false)
    #[serde(default)]
    pub auto_out: bool,
}

fn default_money() -> f64 {
    10.0
}
fn default_pay_type() -> i32 {
    57
}
fn default_park_no() -> String {
    "H51810900057".to_string()
}
fn default_server_ip() -> String {
    "192.168.65.58".to_string()
}

/// HTTP API 统一响应
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

#[derive(Debug, Serialize)]
pub struct PayResultData {
    pub car_no: String,
    pub money: f64,
    pub pay_type: i32,
    pub pay_type_name: String,
    pub order_num: String,
    pub out_trade_no: String,
    pub pay_time: String,
    pub target_park_no: String,
    pub target_server_ip: String,
    pub auto_out_result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server_ip: String,
    pub park_no: String,
    pub box_id: String,
    pub login_name: String,
    pub is_connected: bool,
    pub status_text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SwitchServerRequest {
    pub server_ip: String,
    #[serde(default)]
    pub park_no: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedPopup {
    pub car_no: String,
    pub money: f64,
    pub dsn: String,
    pub in_time: String,
    pub channel_name: String,
    pub popup_time: String,
    pub method: String,
}

pub fn get_pay_type_name(pay_type: i32) -> &'static str {
    match pay_type {
        0 => "现金",
        1 => "微信",
        2 => "支付宝",
        3 => "银联闪付",
        21 => "ETC支付",
        57 => "聚合支付V6(随行付SXF)",
        _ => "第三方支付",
    }
}
