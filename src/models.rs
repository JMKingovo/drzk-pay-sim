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

    /// 车场编号 (留空则采用当前已探测成功的车场；无探测结果时拒绝下发)
    #[serde(default)]
    pub park_no: String,

    /// 目标车场服务器 IP (留空则采用当前已连接的车场)
    #[serde(default)]
    pub server_ip: String,

    /// 通道 DSN (出口扫码必填；场内扫码留空即可)
    #[serde(default)]
    pub dsn: String,

    /// 出口通道名称 (取自负费弹窗识别结果，写入支付记录用；留空则不带通道名)
    #[serde(default)]
    pub channel_name: String,

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

/// HTTP API 统一响应
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

#[derive(Debug, Clone, Serialize)]
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
    /// 是否为出口车道缴费 (false = 场内预缴)
    pub is_lane: bool,
    /// 车场编号是否由本次报文的计费上下文确认为真实入场记录
    pub billing_context_from_park: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server_ip: String,
    pub park_no: String,
    pub box_id: String,
    pub login_name: String,
    pub is_connected: bool,
    pub status_text: String,
    pub mqtt_host: String,
    pub mqtt_user: String,
    pub mqtt_pwd: String,
}

/// 从目标车场服务器数据库探测到的真实信息
///
/// 这些字段全部来自车场本机数据库，任何一项探测不到都会直接报错，
/// 不再回退到写死的车场编号或 MQTT 地址——否则会把支付投递到错误的车场。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkDetails {
    /// 车场编号 (sys_parameters.PARK_NUM)，决定支付报文投递主题
    pub park_no: String,
    /// 在线岗亭号
    pub box_id: String,
    /// 岗亭登录操作员
    pub login_name: String,
    /// 该车场专属支付 MQTT Broker 地址 (host:port)
    pub mqtt_host: String,
    pub mqtt_user: String,
    pub mqtt_pwd: String,
}

/// 界面上提交的 SSH / 数据库凭据 (字段留空或省略表示保持原值不变)
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteCredentials {
    #[serde(default)]
    pub ssh_user: Option<String>,
    #[serde(default)]
    pub ssh_pwd: Option<String>,
    #[serde(default)]
    pub ssh_port: Option<String>,
    #[serde(default)]
    pub db_user: Option<String>,
    #[serde(default)]
    pub db_pwd: Option<String>,
    #[serde(default)]
    pub db_name: Option<String>,
}

/// 界面回显的当前凭据
///
/// 密码只回显"是否已设置"，绝不回显明文——服务监听在 0.0.0.0 上，
/// 明文密码一旦回显，同网段任何人都能通过浏览器直接读走。
#[derive(Debug, Serialize)]
pub struct RemoteConfigView {
    pub ssh_user: String,
    pub ssh_port: String,
    pub db_user: String,
    pub db_name: String,
    pub ssh_pwd_set: bool,
    pub db_pwd_set: bool,
    pub config_file: String,
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
