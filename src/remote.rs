use std::collections::HashMap;
use std::fs;
use std::process::{Command, Stdio};
use std::sync::{OnceLock, RwLock};

use crate::models::{ParkDetails, ParkOverride, RemoteCredentials};

/// 车场服务器的 SSH 与 MySQL 访问凭据
///
/// 工具需要通过 SSH 登录车场服务器、在其本机执行 MySQL 查询来探测车场编号、
/// 支付 MQTT 凭证与通道信息。各车场凭据可能不同，统一在此维护，禁止在业务代码里散落写死。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoteAccess {
    pub ssh_user: String,
    pub ssh_pwd: String,
    pub ssh_port: String,
    pub db_user: String,
    pub db_pwd: String,
    pub db_name: String,
}

impl Default for RemoteAccess {
    fn default() -> Self {
        Self {
            ssh_user: "root".to_string(),
            ssh_pwd: "root".to_string(),
            ssh_port: "22".to_string(),
            db_user: "root".to_string(),
            db_pwd: "123456".to_string(),
            db_name: "ykt".to_string(),
        }
    }
}

impl RemoteAccess {
    /// 转成"全部字段已填写"的凭据形式，用于落盘
    fn to_credentials(&self) -> RemoteCredentials {
        RemoteCredentials {
            ssh_user: Some(self.ssh_user.clone()),
            ssh_pwd: Some(self.ssh_pwd.clone()),
            ssh_port: Some(self.ssh_port.clone()),
            db_user: Some(self.db_user.clone()),
            db_pwd: Some(self.db_pwd.clone()),
            db_name: Some(self.db_name.clone()),
        }
    }

    /// 把界面上提交的字段覆盖到当前配置 (空值表示不修改该项)
    pub fn apply_update(&mut self, up: &RemoteCredentials) {
        overlay(self, up);
    }

    /// 密码是否仍停留在内置默认值 (仅用于启动提示，不对外暴露)
    pub fn ssh_pwd_is_builtin(&self) -> bool {
        self.ssh_pwd == RemoteAccess::default().ssh_pwd
    }

    pub fn db_pwd_is_builtin(&self) -> bool {
        self.db_pwd == RemoteAccess::default().db_pwd
    }
}

/// 参数与环境变量名对照 (环境变量优先级低于命令行参数)
const ENV_KEYS: [(&str, &str); 6] = [
    ("--ssh-user", "DRZK_SSH_USER"),
    ("--ssh-pwd", "DRZK_SSH_PWD"),
    ("--ssh-port", "DRZK_SSH_PORT"),
    ("--db-user", "DRZK_DB_USER"),
    ("--db-pwd", "DRZK_DB_PWD"),
    ("--db-name", "DRZK_DB_NAME"),
];

static REMOTE: RwLock<Option<Store>> = RwLock::new(None);

/// 内存中的配置：SSH/数据库凭据 + 各车场的手填参数
#[derive(Debug, Clone, Default)]
struct Store {
    remote: RemoteAccess,
    parks: HashMap<String, ParkOverride>,
}

/// 读取当前生效凭据 (未初始化时返回内置默认值)
pub fn current() -> RemoteAccess {
    REMOTE
        .read()
        .unwrap()
        .clone()
        .map(|s| s.remote)
        .unwrap_or_else(RemoteAccess::default)
}

/// 覆盖当前生效凭据 (界面保存时调用)
pub fn set(cfg: RemoteAccess) {
    let mut lock = REMOTE.write().unwrap();
    let parks = lock.take().map(|s| s.parks).unwrap_or_default();
    *lock = Some(Store { remote: cfg, parks });
}

/// 取某个车场已保存的手填参数
pub fn park_override(server_ip: &str) -> Option<ParkOverride> {
    REMOTE
        .read()
        .unwrap()
        .as_ref()
        .and_then(|s| s.parks.get(server_ip).cloned())
}

/// 已保存手填值的车场 IP 列表
pub fn saved_parks() -> Vec<String> {
    let mut v: Vec<String> = REMOTE
        .read()
        .unwrap()
        .as_ref()
        .map(|s| s.parks.keys().cloned().collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// 合并保存某个车场的手填参数 (空值不覆盖已有值)
pub fn update_park_override(server_ip: &str, up: &crate::models::ParkConfigUpdate) -> Result<ParkOverride, String> {
    let mut lock = REMOTE.write().unwrap();
    let store = lock.get_or_insert_with(|| Store {
        remote: RemoteAccess::default(),
        parks: HashMap::new(),
    });

    let entry = store.parks.entry(server_ip.to_string()).or_default();

    // 选择"自动跟随"时取消指定岗亭
    if up.auto_booth == Some(true) {
        entry.box_id = None;
        entry.login_name = None;
        entry.booth_pinned = Some(false);
    } else if up.booth_pinned == Some(true) {
        // 界面明确指定了岗亭
        entry.booth_pinned = Some(true);
    }

    let pairs: [(&Option<String>, &mut Option<String>); 6] = [
        (&up.park_no, &mut entry.park_no),
        (&up.mqtt_host, &mut entry.mqtt_host),
        (&up.mqtt_user, &mut entry.mqtt_user),
        (&up.mqtt_pwd, &mut entry.mqtt_pwd),
        (&up.box_id, &mut entry.box_id),
        (&up.login_name, &mut entry.login_name),
    ];
    for (src, dst) in pairs {
        if let Some(v) = src {
            if !v.trim().is_empty() {
                *dst = Some(v.clone());
            }
        }
    }

    let merged = entry.clone();
    let snapshot = store.clone();
    drop(lock);

    save_store(&snapshot)?;
    Ok(merged)
}

/// 记下某个车场已知的岗亭清单 (供界面下拉选择，没有 SSH 时也能选)
pub fn remember_booths(server_ip: &str, booths: &[crate::models::Booth]) -> Result<(), String> {
    if booths.is_empty() {
        return Ok(());
    }

    let mut lock = REMOTE.write().unwrap();
    let store = lock.get_or_insert_with(|| Store {
        remote: RemoteAccess::default(),
        parks: HashMap::new(),
    });
    store
        .parks
        .entry(server_ip.to_string())
        .or_default()
        .booths = booths.to_vec();
    let snapshot = store.clone();
    drop(lock);

    save_store(&snapshot)
}

/// 探测成功后把手填值更新为最新探测结果 (下次没有 SSH 时可直接使用)
pub fn remember_probed(server_ip: &str, details: &ParkDetails) -> Result<(), String> {
    let mut lock = REMOTE.write().unwrap();
    let store = lock.get_or_insert_with(|| Store {
        remote: RemoteAccess::default(),
        parks: HashMap::new(),
    });
    let entry = store.parks.entry(server_ip.to_string()).or_default();
    entry.merge_probed(details);
    let snapshot = store.clone();
    drop(lock);

    save_store(&snapshot)
}

/// 配置文件路径：可由 DRZK_CONFIG_FILE 指定，默认放在用户目录下
///
/// 放在用户目录而非程序目录，避免这个含明文密码的文件被误提交进代码仓库。
/// Windows 上没有 HOME(是 USERPROFILE)，两者都取不到时退到程序所在目录。
pub fn config_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("DRZK_CONFIG_FILE") {
        if !p.trim().is_empty() {
            return std::path::PathBuf::from(p);
        }
    }

    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return std::path::PathBuf::from(v).join(".drzk-pay-sim.json");
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join(".drzk-pay-sim.json");
        }
    }

    std::path::PathBuf::from(".drzk-pay-sim.json")
}

/// 配置文件结构：SSH/数据库凭据扁平存放，另加 parks 表存放各车场手填参数
///
/// 旧版配置文件(只有扁平凭据、没有 parks)仍可正常读取。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ConfigFile {
    #[serde(flatten)]
    remote: RemoteCredentials,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    parks: HashMap<String, ParkOverride>,
}

/// 读取配置文件；文件不存在或格式错误时返回 None
fn load_file() -> Option<Store> {
    let text = fs::read_to_string(config_path()).ok()?;
    let file: ConfigFile = serde_json::from_str(&text).ok()?;

    let mut remote = RemoteAccess::default();
    overlay(&mut remote, &file.remote);
    Some(Store { remote, parks: file.parks })
}

/// 把当前凭据落盘保存 (文件权限收紧为 0600)，保留已存的车场手填值
pub fn save_file(cfg: &RemoteAccess) -> Result<(), String> {
    let parks = REMOTE
        .read()
        .unwrap()
        .as_ref()
        .map(|s| s.parks.clone())
        .unwrap_or_default();

    save_store(&Store { remote: cfg.clone(), parks })
}

fn save_store(store: &Store) -> Result<(), String> {
    let file = ConfigFile {
        remote: store.remote.to_credentials(),
        parks: store.parks.clone(),
    };
    let path = config_path();
    let text = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化失败: {}", e))?;
    fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {}", path.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

/// 启动时初始化配置：配置文件 → 环境变量 → 命令行参数 (后者覆盖前者)
pub fn init(args: &[String]) {
    let mut store = load_file().unwrap_or_default();

    for (_, env_key) in ENV_KEYS {
        if let Ok(val) = std::env::var(env_key) {
            if !val.is_empty() {
                apply(&mut store.remote, env_key, val);
            }
        }
    }

    let mut i = 0;
    while i < args.len() {
        if let Some((flag, _)) = ENV_KEYS.iter().find(|(flag, _)| *flag == args[i]) {
            if i + 1 < args.len() {
                apply(&mut store.remote, flag, args[i + 1].clone());
            }
        }
        i += 1;
    }

    *REMOTE.write().unwrap() = Some(store);
}

fn overlay(cfg: &mut RemoteAccess, up: &RemoteCredentials) {
    let pairs: [(&Option<String>, &mut String); 6] = [
        (&up.ssh_user, &mut cfg.ssh_user),
        (&up.ssh_pwd, &mut cfg.ssh_pwd),
        (&up.ssh_port, &mut cfg.ssh_port),
        (&up.db_user, &mut cfg.db_user),
        (&up.db_pwd, &mut cfg.db_pwd),
        (&up.db_name, &mut cfg.db_name),
    ];
    for (src, dst) in pairs {
        if let Some(v) = src {
            if !v.is_empty() {
                *dst = v.clone();
            }
        }
    }
}

fn apply(cfg: &mut RemoteAccess, key: &str, val: String) {
    let slot = match key {
        "--ssh-user" | "DRZK_SSH_USER" => &mut cfg.ssh_user,
        "--ssh-pwd" | "DRZK_SSH_PWD" => &mut cfg.ssh_pwd,
        "--ssh-port" | "DRZK_SSH_PORT" => &mut cfg.ssh_port,
        "--db-user" | "DRZK_DB_USER" => &mut cfg.db_user,
        "--db-pwd" | "DRZK_DB_PWD" => &mut cfg.db_pwd,
        "--db-name" | "DRZK_DB_NAME" => &mut cfg.db_name,
        _ => return,
    };
    *slot = val;
}

/// 远程命令的失败类型
enum Failure {
    /// SSH 自身连不上或认证失败 (偶发抖动，值得重试)
    Ssh(String),
    /// 远程命令执行失败 (例如 MySQL 认证错误，重试没有意义)
    Command(String),
}

/// SSH 偶发抖动的重试次数
const MAX_ATTEMPTS: usize = 3;

/// 在目标车场服务器上执行一条 shell 命令并返回标准输出
///
/// 现场出现过连续两条查询里第二条 SSH 认证失败的情况 (sshpass 并发抖动)，
/// 因此对 SSH 层面的失败做有限重试，避免偶发抖动让出口监听功能被误判为不可用。
pub fn run_command(server_ip: &str, remote_cmd: &str) -> Result<String, String> {
    let mut last = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        match run_command_once(server_ip, remote_cmd) {
            Ok(out) => return Ok(out),
            Err(Failure::Command(msg)) => return Err(msg),
            Err(Failure::Ssh(msg)) => {
                last = msg;
                if attempt < MAX_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
            }
        }
    }

    Err(last)
}

/// SSH 执行方式
///
/// Windows 上没有 sshpass，因此按可用性自动降级：sshpass → plink(PuTTY) → 系统 ssh(密钥认证)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transport {
    Sshpass,
    Plink,
    PlainSsh,
}

static TRANSPORT: OnceLock<Transport> = OnceLock::new();

pub fn transport() -> Transport {
    *TRANSPORT.get_or_init(|| {
        if cmd_available("sshpass") {
            Transport::Sshpass
        } else if cmd_available("plink") {
            Transport::Plink
        } else {
            Transport::PlainSsh
        }
    })
}

pub fn transport_name() -> &'static str {
    match transport() {
        Transport::Sshpass => "sshpass + ssh",
        Transport::Plink => "plink (PuTTY)",
        Transport::PlainSsh => "ssh (密钥/agent 认证，不使用密码)",
    }
}

/// 命令能否执行 (能 spawn 即视为存在，退出码无关)
fn cmd_available(name: &str) -> bool {
    Command::new(name)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok()
}

/// 按当前执行方式拼出「程序 + 参数」
fn build_command(cfg: &RemoteAccess, target: &str, remote_cmd: &str) -> (String, Vec<String>) {
    match transport() {
        Transport::Sshpass => (
            "sshpass".to_string(),
            vec![
                "-p".into(),
                cfg.ssh_pwd.clone(),
                "ssh".into(),
                "-p".into(),
                cfg.ssh_port.clone(),
                "-o".into(),
                "StrictHostKeyChecking=no".into(),
                "-o".into(),
                "ConnectTimeout=3".into(),
                target.into(),
                remote_cmd.into(),
            ],
        ),
        Transport::Plink => (
            "plink".to_string(),
            vec![
                "-pw".into(),
                cfg.ssh_pwd.clone(),
                "-P".into(),
                cfg.ssh_port.clone(),
                "-batch".into(),
                target.into(),
                remote_cmd.into(),
            ],
        ),
        Transport::PlainSsh => (
            "ssh".to_string(),
            vec![
                "-p".into(),
                cfg.ssh_port.clone(),
                "-o".into(),
                "StrictHostKeyChecking=no".into(),
                "-o".into(),
                "ConnectTimeout=3".into(),
                "-o".into(),
                "BatchMode=yes".into(),
                target.into(),
                remote_cmd.into(),
            ],
        ),
    }
}

fn run_command_once(server_ip: &str, remote_cmd: &str) -> Result<String, Failure> {
    let cfg = current();
    let target = format!("{}@{}", cfg.ssh_user, server_ip);
    let (program, args) = build_command(&cfg, &target, remote_cmd);

    let output = Command::new(&program)
        .args(&args)
        .output()
        .map_err(|e| {
            Failure::Ssh(format!(
                "无法执行 {} ({}): 请安装 sshpass (macOS/Linux) 或 plink/PuTTY (Windows)，\
                 或改用 SSH 密钥认证后仅用系统 ssh",
                program, e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let code = output.status.code().unwrap_or(-1);

        // stderr 里出现这些字样，说明是 SSH 自身连不上或认证失败
        let ssh_level = [
            "Permission denied",
            "Connection refused",
            "Connection timed out",
            "Operation timed out",
            "Could not resolve",
            "No route to host",
            "Host key verification failed",
        ]
        .iter()
        .any(|p| stderr.contains(p));

        if ssh_level {
            return Err(Failure::Ssh(format!(
                "SSH 登录 {} 失败 (账号 {}): {}",
                server_ip,
                cfg.ssh_user,
                last_line(&stderr)
            )));
        }

        // 否则是远程命令自身失败 (例如 MySQL 认证失败)，把它的报错原样带回来
        let detail = if !stdout.trim().is_empty() {
            strip_warnings(&stdout)
        } else {
            last_line(&stderr).to_string()
        };
        return Err(Failure::Command(format!(
            "远程命令执行失败 (退出码 {}): {}",
            code, detail
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 取最后一行非空内容作为错误摘要
fn last_line(text: &str) -> &str {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or("无错误输出")
        .trim()
}

/// 去掉 mysql 命令行密码告警等无意义噪音行
fn strip_warnings(text: &str) -> String {
    text.lines()
        .filter(|l| !l.contains("Using a password on the command line interface"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// 在目标车场服务器本机执行一条 MySQL 查询，返回制表符分隔的结果文本
pub fn run_sql(server_ip: &str, sql: &str) -> Result<String, String> {
    let cfg = current();
    // 合并 stderr，让 MySQL 的报错 (如 Access denied) 能原样回传给使用者
    let remote_cmd = format!(
        "PAGER=cat mysql -u{} -p{} {} -B -e \"{}\" 2>&1",
        shell_quote(&cfg.db_user),
        shell_quote(&cfg.db_pwd),
        shell_quote(&cfg.db_name),
        sql
    );

    let stdout = run_command(server_ip, &remote_cmd)?;

    // mysql 的报错被合并进来了，结果里没有表头即说明查询未生效
    if stdout.trim().is_empty() {
        return Err(format!(
            "MySQL 查询无任何返回，请检查数据库账号密码与库名 (当前: {}@{}/{})",
            cfg.db_user, server_ip, cfg.db_name
        ));
    }

    Ok(stdout)
}

/// 把「查询结果文本」解析为制表符分隔的字段
///
/// 需要同时跳过 mysql 的命令行密码告警行与结果表头，
/// 否则表头会混进数据里 (例如把岗亭号解析成字面量 "box_id")。
pub fn parse_rows(output: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.starts_with("mysql: [Warning]"))
        .map(|l| l.split('\t').map(|f| f.trim().to_string()).collect())
        .collect();

    // 首行是字段名表头，丢弃
    if !rows.is_empty() {
        rows.remove(0);
    }

    rows
}

/// shell 单引号安全包装 (含单引号自身转义)
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(fields: [Option<&str>; 6]) -> RemoteCredentials {
        RemoteCredentials {
            ssh_user: fields[0].map(String::from),
            ssh_pwd: fields[1].map(String::from),
            ssh_port: fields[2].map(String::from),
            db_user: fields[3].map(String::from),
            db_pwd: fields[4].map(String::from),
            db_name: fields[5].map(String::from),
        }
    }

    #[test]
    fn 解析结果时跳过表头() {
        let out = "box_id\tloginName\n1\t超级管理员\n";
        let rows = parse_rows(out);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec!["1", "超级管理员"]);
    }

    #[test]
    fn 解析结果时跳过mysql警告行() {
        let out = "mysql: [Warning] Using a password on the command line interface can be insecure.\n\
                   parameter_code\tparameter_value\nPARK_NUM\tX51810900043\n";
        let rows = parse_rows(out);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec!["PARK_NUM", "X51810900043"]);
    }

    #[test]
    fn 空结果不会构造出脏行() {
        let out = "box_id\tloginName\n";
        assert!(parse_rows(out).is_empty());
        assert!(parse_rows("").is_empty());
    }

    #[test]
    fn 含空格的字段保持完整() {
        let out = "channel_ip\tdsn\n192.168.151.47\tSIM21712648130131729\n";
        let rows = parse_rows(out);
        assert_eq!(rows[0], vec!["192.168.151.47", "SIM21712648130131729"]);
    }

    #[test]
    fn 界面提交只覆盖填写的字段() {
        let mut cfg = RemoteAccess::default();
        cfg.ssh_pwd = "old-ssh".into();
        cfg.db_pwd = "old-db".into();

        // 只改了 SSH 密码，数据库密码必须原样保留
        cfg.apply_update(&creds([
            None,
            Some("new-ssh"),
            None,
            None,
            None,
            None,
        ]));

        assert_eq!(cfg.ssh_pwd, "new-ssh");
        assert_eq!(cfg.db_pwd, "old-db");
        assert_eq!(cfg.ssh_user, RemoteAccess::default().ssh_user);
    }

    #[test]
    fn 界面提交的空字符串不会清空已有值() {
        let mut cfg = RemoteAccess::default();
        cfg.db_pwd = "keep-me".into();

        cfg.apply_update(&creds([None, None, None, None, Some(""), None]));

        assert_eq!(cfg.db_pwd, "keep-me");
    }
}
