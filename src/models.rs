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
    /// 车场参数来源: 自动探测 / 手动配置 / 未配置
    #[serde(default)]
    pub park_config_source: String,
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
    /// 该车场已知的岗亭清单 (供界面下拉选择)
    #[serde(default)]
    pub booths: Vec<Booth>,
}

/// 车场的一个岗亭
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Booth {
    pub box_id: String,
    pub login_name: String,
    #[serde(default)]
    pub online: bool,
}

/// 通过 HTTP 从车场服务直接读到的车场信息 (无需 SSH)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkHttpInfo {
    /// 车场编号 (接口里的 projectNo)，就是支付报文的投递主题所用的编号
    pub park_no: String,
    /// 车场名称 (接口里的 projectName)
    pub project_name: String,
    /// 车场开启的支付方式编码，如 "1,2,34,57"
    pub payment: String,
}

/// 界面上提交的 SSH / 数据库凭据 (字段留空或省略表示保持原值不变)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_pwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_pwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_name: Option<String>,
}

/// 某个车场的手填参数 (按车场 IP 保存)
///
/// 车场编号与支付 MQTT 凭证本来只能通过 SSH+MySQL 从车场数据库探测；
/// 存下这份手填值后，没有 SSH 的机器 (例如 Windows 单机跑 exe) 也能完整使用支付功能。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParkOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub park_no: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mqtt_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mqtt_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mqtt_pwd: Option<String>,
    /// 岗亭号 (缺失时仅影响出口弹窗监听与开闸联动，不影响支付)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_name: Option<String>,
    /// 岗亭是否为"用户在界面上指定"的
    ///
    /// box_id/login_name 也可能是旧版本由探测写下的，那种不算指定；
    /// 只有这里为 true 时才是用户的明确选择，才禁止自动跟随。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booth_pinned: Option<bool>,
    /// 该车场已知的岗亭清单 (探测到后记下，供界面下拉选择；没有 SSH 时也能选)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub booths: Vec<Booth>,
}

impl ParkOverride {
    /// 用户是否明确指定了岗亭 (只有这种情况才禁止自动跟随)
    pub fn is_pinned(&self) -> bool {
        self.booth_pinned.unwrap_or(false)
            && self.box_id.as_deref().map_or(false, |v| !v.trim().is_empty())
    }

    /// 拼成车场信息 (缺项留空，由调用方判断够不够用)
    ///
    /// 岗亭：优先用界面指定的；没指定就用已知清单里推荐的（清单按在线优先排序）。
    /// 这样没有 SSH 时也能连上正确的岗亭。
    pub fn to_details(&self) -> ParkDetails {
        let (box_id, login_name) = if self.is_pinned() {
            (
                self.box_id.clone().unwrap_or_default(),
                self.login_name.clone().unwrap_or_default(),
            )
        } else {
            match self.booths.first() {
                Some(b) => (b.box_id.clone(), b.login_name.clone()),
                None => (String::new(), String::new()),
            }
        };

        ParkDetails {
            park_no: self.park_no.clone().unwrap_or_default(),
            box_id,
            login_name,
            mqtt_host: self.mqtt_host.clone().unwrap_or_default(),
            mqtt_user: self.mqtt_user.clone().unwrap_or_default(),
            mqtt_pwd: self.mqtt_pwd.clone().unwrap_or_default(),
            booths: self.booths.clone(),
        }
    }

    /// 是否足以支撑支付下发 (车场编号 + MQTT 地址)
    pub fn can_pay(&self) -> bool {
        self.park_no.as_deref().map_or(false, |v| !v.trim().is_empty())
            && self.mqtt_host.as_deref().map_or(false, |v| !v.trim().is_empty())
    }

    /// 是否足以支撑出口弹窗监听
    ///
    /// 指定了完整岗亭(号+操作员)，或已知清单里有可用岗亭，都算可用。
    pub fn can_monitor(&self) -> bool {
        let pinned = self.is_pinned()
            && self.login_name.as_deref().map_or(false, |v| !v.trim().is_empty());
        pinned || self.booths.first().is_some()
    }

    /// 把探测到的值合并进来 (探测值优先，作为最新的已知正确值存下)
    pub fn merge_probed(&mut self, d: &ParkDetails) {
        self.park_no = Some(d.park_no.clone());
        self.mqtt_host = Some(d.mqtt_host.clone());
        self.mqtt_user = Some(d.mqtt_user.clone());
        self.mqtt_pwd = Some(d.mqtt_pwd.clone());
        // 注意：这里不动 box_id/login_name —— 它们代表"用户在界面上指定的岗亭"，
        // 不能被探测结果覆盖，否则指定就失效了。探测到的岗亭进 booths 清单，
        // 未指定时由清单里推荐的（在线优先）承担连接目标。
        // 岗亭清单同样只在探测到内容时更新，避免把已知清单清空
        if !d.booths.is_empty() {
            self.booths = d.booths.clone();
        }
    }

    /// 是否有手填值与探测值不一致 (用于界面提示，避免静默以某一方为准)
    pub fn conflicts_with(&self, d: &ParkDetails) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(v) = self.park_no.as_deref() {
            if !v.is_empty() && v != d.park_no {
                out.push(format!("车场编号 手填 {} / 探测 {}", v, d.park_no));
            }
        }
        if let Some(v) = self.mqtt_host.as_deref() {
            if !v.is_empty() && v != d.mqtt_host {
                out.push(format!("MQTT 地址 手填 {} / 探测 {}", v, d.mqtt_host));
            }
        }
        out
    }
}

/// 界面上的车场参数视图 (密码只回显是否已设置)
#[derive(Debug, Serialize)]
pub struct ParkConfigView {
    pub server_ip: String,
    /// 当前生效值
    pub park_no: String,
    pub mqtt_host: String,
    pub mqtt_user: String,
    pub mqtt_pwd_set: bool,
    pub box_id: String,
    pub login_name: String,
    /// 当前生效值的来源: 自动探测 / 手动配置 / 未配置
    pub source: String,
    /// 已保存的手填值 (用于回显对比)
    pub manual_park_no: String,
    pub manual_mqtt_host: String,
    pub manual_mqtt_user: String,
    pub manual_mqtt_pwd_set: bool,
    pub manual_box_id: String,
    pub manual_login_name: String,
    /// 已保存手填值的车场列表
    pub saved_parks: Vec<String>,
    /// 当前岗亭是否为用户指定
    pub booth_pinned: bool,
    /// 该车场已知的岗亭清单 (供下拉选择)
    pub booths: Vec<Booth>,
}

/// 界面提交的车场参数 (留空表示不修改)
#[derive(Debug, Clone, Deserialize)]
pub struct ParkConfigUpdate {
    pub server_ip: String,
    #[serde(default)]
    pub park_no: Option<String>,
    #[serde(default)]
    pub mqtt_host: Option<String>,
    #[serde(default)]
    pub mqtt_user: Option<String>,
    #[serde(default)]
    pub mqtt_pwd: Option<String>,
    #[serde(default)]
    pub box_id: Option<String>,
    #[serde(default)]
    pub login_name: Option<String>,
    /// true = 取消指定岗亭，改为自动跟随车场在线岗亭
    #[serde(default)]
    pub auto_booth: Option<bool>,
    /// true = 本次提交的岗亭是"指定"（配合 box_id/login_name）
    #[serde(default)]
    pub booth_pinned: Option<bool>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn details(park_no: &str, mqtt: &str, booth: &str) -> ParkDetails {
        ParkDetails {
            park_no: park_no.to_string(),
            box_id: booth.to_string(),
            login_name: "超级管理员".to_string(),
            mqtt_host: mqtt.to_string(),
            mqtt_user: "u".to_string(),
            mqtt_pwd: "p".to_string(),
            booths: Vec::new(),
        }
    }

    #[test]
    fn 手填值够不够支付看编号与mqtt地址() {
        let mut m = ParkOverride::default();
        assert!(!m.can_pay(), "全空不该允许支付");
        m.park_no = Some("H51810900057".into());
        assert!(!m.can_pay(), "只有编号仍不够");
        m.mqtt_host = Some("a.drzk.cn:62181".into());
        assert!(m.can_pay(), "编号+地址即可支付");
    }

    #[test]
    fn 手填值够不够监听看岗亭与操作员() {
        let mut m = ParkOverride::default();
        assert!(!m.can_monitor());
        m.box_id = Some("1".into());
        assert!(!m.can_monitor());
        m.login_name = Some("超级管理员".into());
        assert!(!m.can_monitor(), "旧数据里可能残留岗亭值，但没有指定标记就不算指定");
        m.booth_pinned = Some(true);
        assert!(m.can_monitor(), "明确指定后才算可用");
    }

    #[test]
    fn 旧配置里的岗亭值不会被当成指定() {
        // 模拟旧版本探测写下的值：有 box_id/login_name，没有 booth_pinned
        let mut m = ParkOverride::default();
        m.box_id = Some("1".into());
        m.login_name = Some("超级管理员".into());
        assert!(!m.is_pinned(), "旧数据不该被当成用户指定，否则自动跟随会失效");

        m.booths = vec![Booth { box_id: "9".into(), login_name: "新岗".into(), online: true }];
        assert_eq!(m.to_details().box_id, "9", "未指定时应采用清单里推荐的岗亭");
    }

    #[test]
    fn 探测不会覆盖用户指定的岗亭() {
        let mut m = ParkOverride::default();
        m.box_id = Some("2".into());
        m.login_name = Some("操作员B".into());

        m.merge_probed(&details("P", "h:1", "1"));

        assert_eq!(m.box_id.as_deref(), Some("2"), "指定岗亭被探测结果覆盖了");
        assert_eq!(m.login_name.as_deref(), Some("操作员B"));
        assert_eq!(m.booths.len(), 0, "探测没给清单时不该凭空造");
    }

    #[test]
    fn 未指定岗亭时用清单里推荐的() {
        let mut m = ParkOverride::default();
        m.booths = vec![
            Booth { box_id: "1".into(), login_name: "在线岗".into(), online: true },
            Booth { box_id: "2".into(), login_name: "离线岗".into(), online: false },
        ];
        assert!(m.can_monitor(), "有清单就该能监听");
        let d = m.to_details();
        assert_eq!(d.box_id, "1", "应取清单里在线优先的第一个");
        assert_eq!(d.login_name, "在线岗");
    }

    #[test]
    fn 探测结果会覆盖手填值但不抹掉岗亭() {
        let mut m = ParkOverride::default();
        m.park_no = Some("OLD".into());
        m.box_id = Some("1".into());
        m.login_name = Some("超级管理员".into());

        // 探测没拿到岗亭时，不能把已有的岗亭信息清掉
        let mut d = details("NEW", "newhost:1883", "");
        d.login_name = String::new();
        m.merge_probed(&d);

        assert_eq!(m.park_no.as_deref(), Some("NEW"));
        assert_eq!(m.mqtt_host.as_deref(), Some("newhost:1883"));
        assert_eq!(m.box_id.as_deref(), Some("1"));
        assert_eq!(m.login_name.as_deref(), Some("超级管理员"));
    }

    #[test]
    fn 手填值与探测值不一致会被报出来() {
        let mut m = ParkOverride::default();
        m.park_no = Some("MANUAL".into());
        m.mqtt_host = Some("a.drzk.cn:62181".into());

        let same = details("MANUAL", "a.drzk.cn:62181", "1");
        assert!(m.conflicts_with(&same).is_empty(), "一致时不该报冲突");

        let diff = details("PROBED", "b.drzk.cn:62181", "1");
        let c = m.conflicts_with(&diff);
        assert_eq!(c.len(), 2, "编号与地址都不同，应报两条: {:?}", c);
    }

    #[test]
    fn 手填值可拼成车场信息() {
        let mut m = ParkOverride::default();
        m.park_no = Some("P".into());
        let d = m.to_details();
        assert_eq!(d.park_no, "P");
        assert_eq!(d.mqtt_host, "", "未填项应为空，交给调用方判断够不够用");
    }
}
