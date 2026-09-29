#![recursion_limit = "256"]

mod http_server;
mod models;
mod mqtt;
mod remote;
mod service;
mod ws_listener;

use std::env;
use std::sync::{Arc, Mutex};
use models::PayRequest;
use service::execute_payment;
use ws_listener::start_ws_listener;

fn main() {
    let args: Vec<String> = env::args().collect();

    // 如果指定了 --help 或 -h
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    // 只要没有显式指定 --car，就启动图形化界面与 HTTP 服务模式
    let has_car_arg = args.iter().any(|a| a == "--car" || a == "-c");
    let is_serve_mode = !has_car_arg || args.iter().any(|a| a == "--serve" || a == "-s");

    if is_serve_mode {
        let mut port = "18766".to_string();
        let mut server_ip = String::new();
        let mut no_open = false;
        for i in 0..args.len() {
            if (args[i] == "--port" || args[i] == "-p") && i + 1 < args.len() {
                port = args[i + 1].clone();
            }
            if args[i] == "--server" && i + 1 < args.len() {
                server_ip = args[i + 1].clone();
            }
            if args[i] == "--no-open" {
                no_open = true;
            }
        }

        remote::init(&args);
        print_remote_access(&server_ip);

        let mut cfg = models::ServerConfig {
            server_ip: server_ip.clone(),
            park_no: String::new(),
            box_id: String::new(),
            login_name: String::new(),
            is_connected: false,
            status_text: if server_ip.is_empty() {
                "尚未选择车场，请输入车场 IP 并点击「连接车场」".to_string()
            } else {
                format!("车场 {} 信息未配置，请点击「连接车场」", server_ip)
            },
            mqtt_host: String::new(),
            mqtt_user: String::new(),
            mqtt_pwd: String::new(),
            park_config_source: "未配置".to_string(),
        };

        // 优先自动探测(SSH+MySQL)；探测不通时退回该车场已保存的手填参数；
        // 两者都没有就保持未配置并禁用支付，绝不凭空猜测
        if !server_ip.is_empty() {
            match ws_listener::probe_server_details(&server_ip) {
                Ok(mut d) => {
                    println!(
                        "  ✅ 车场探测成功: 编号 {}, 岗亭 {}, 操作员 {}",
                        d.park_no,
                        if d.box_id.is_empty() { "无" } else { &d.box_id },
                        d.login_name
                    );
                    if let Err(e) = remote::remember_probed(&server_ip, &d) {
                        eprintln!("  ⚠️ 保存车场参数失败: {}", e);
                    }

                    // 界面指定了岗亭就优先用它
                    let pin_note = ws_listener::apply_pinned_booth(&mut d, remote::park_override(&server_ip).as_ref());
                    if let Some(n) = &pin_note {
                        println!("  ⚠️ 岗亭: {}", n);
                    }

                    let no_booth = d.box_id.is_empty();
                    cfg.park_no = d.park_no.clone();
                    cfg.box_id = d.box_id;
                    cfg.login_name = d.login_name;
                    cfg.mqtt_host = d.mqtt_host;
                    cfg.mqtt_user = d.mqtt_user;
                    cfg.mqtt_pwd = d.mqtt_pwd;
                    cfg.park_config_source = "自动探测".to_string();
                    cfg.status_text = if no_booth {
                        format!("车场 {} 已探测 (编号 {})，但未取到岗亭信息，无法监听出口出车；可重新点击「连接车场」重试", server_ip, d.park_no)
                    } else {
                        format!("已探测 {}:8089 (车场: {})，正在连接通道...", server_ip, d.park_no)
                    };
                }
                Err(e) => {
                    eprintln!("  ⚠️ 车场探测失败: {}", e);
                    match remote::park_override(&server_ip) {
                        Some(m) if m.can_pay() => {
                            let d = m.to_details();
                            println!("  → 改用该车场已保存的手填参数接入 (未经探测): 编号 {}", d.park_no);
                            let no_booth = d.box_id.is_empty() || d.login_name.is_empty();
                            cfg.park_no = d.park_no.clone();
                            cfg.box_id = d.box_id;
                            cfg.login_name = d.login_name;
                            cfg.mqtt_host = d.mqtt_host;
                            cfg.mqtt_user = d.mqtt_user;
                            cfg.mqtt_pwd = d.mqtt_pwd;
                            cfg.park_config_source = "手动配置".to_string();
                            cfg.status_text = if no_booth {
                                format!("已按手填参数接入 {} (车场 {})，但未填岗亭，无法监听出口出车", server_ip, d.park_no)
                            } else {
                                format!("已按手填参数接入 {} (车场 {}, 未经探测)，正在连接通道...", server_ip, d.park_no)
                            };
                        }
                        _ => {
                            eprintln!("     支付下发已被禁用。可在界面顶部「车场参数」填好该车场的车场编号与 MQTT 凭证，或修正 SSH/数据库凭据后重连。");
                            cfg.status_text = format!("车场 {} 未配置，支付下发已禁用", server_ip);
                        }
                    }
                }
            }
        }

        let shared_config = Arc::new(Mutex::new(cfg));

        let shared_popup = Arc::new(Mutex::new(None));
        let shared_history: ws_listener::SharedPayHistory = Arc::new(Mutex::new(Vec::new()));
        start_ws_listener(shared_config.clone(), shared_popup.clone());
        // 岗亭自动跟随：定期复查车场在线岗亭，变了就切过去（界面指定了岗亭则不切）
        ws_listener::start_booth_watcher(shared_config.clone(), 30);

        let browser_url = format!("http://127.0.0.1:{}", port);
        if !no_open {
            let url_clone = browser_url.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                #[cfg(target_os = "macos")]
                { std::process::Command::new("open").arg(&url_clone).spawn().ok(); }
                #[cfg(target_os = "windows")]
                { std::process::Command::new("cmd").args(&["/c", "start", &url_clone]).spawn().ok(); }
                #[cfg(target_os = "linux")]
                { std::process::Command::new("xdg-open").arg(&url_clone).spawn().ok(); }
            });
        }

        let addr = format!("0.0.0.0:{}", port);
        http_server::start_http_server(&addr, shared_config, shared_popup, shared_history);
        return;
    }

    // 否则作为 CLI 命令行工具直接下发
    remote::init(&args);

    let mut car_no = String::new();
    let mut money = 10.0;
    let mut pay_type = 57;
    let mut server_ip = String::new();
    let mut park_no_arg: Option<String> = None;
    let mut dsn = String::new();
    let mut auto_out = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--car" | "-c" => {
                if i + 1 < args.len() {
                    car_no = args[i + 1].clone();
                    i += 1;
                }
            }
            "--money" | "-m" => {
                if i + 1 < args.len() {
                    money = args[i + 1].parse().unwrap_or(10.0);
                    i += 1;
                }
            }
            "--pay-type" | "-t" => {
                if i + 1 < args.len() {
                    pay_type = args[i + 1].parse().unwrap_or(57);
                    i += 1;
                }
            }
            "--server" => {
                if i + 1 < args.len() {
                    server_ip = args[i + 1].clone();
                    i += 1;
                }
            }
            "--park-no" => {
                if i + 1 < args.len() {
                    park_no_arg = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--dsn" => {
                if i + 1 < args.len() {
                    dsn = args[i + 1].clone();
                    i += 1;
                }
            }
            "--auto-out" => {
                auto_out = true;
            }
            _ => {}
        }
        i += 1;
    }

    if car_no.is_empty() {
        eprintln!("【错误】请提供车牌号，例如: --car 粤B88801\n使用 --help 查看完整帮助。");
        std::process::exit(1);
    }

    if server_ip.is_empty() {
        eprintln!("【错误】请用 --server <车场IP> 指定目标车场服务器，例如: --server 192.168.65.59");
        std::process::exit(1);
    }

    print_remote_access(&server_ip);

    // 探测优先；不通时用该车场已保存的手填参数，两者都没有则报错退出
    let values = match ws_listener::probe_server_details(&server_ip) {
        Ok(d) => {
            if let Err(e) = remote::remember_probed(&server_ip, &d) {
                eprintln!("  ⚠️ 保存车场参数失败: {}", e);
            }
            d
        }
        Err(e) => {
            eprintln!("  ⚠️ 车场探测失败: {}", e);
            match remote::park_override(&server_ip) {
                Some(m) if m.can_pay() => {
                    println!("  → 改用该车场已保存的手填参数 (未经探测)");
                    m.to_details()
                }
                _ => {
                    eprintln!("【错误】该车场既探测不到、也没有手填参数。");
                    eprintln!("       请在界面「车场参数」中填好车场编号与 MQTT 凭证，或指定 SSH/数据库凭据:");
                    eprintln!("       --ssh-user/--ssh-pwd/--ssh-port/--db-user/--db-pwd/--db-name");
                    std::process::exit(1);
                }
            }
        }
    };

    let park_no = park_no_arg.unwrap_or_else(|| values.park_no.clone());
    let (mqtt_host, mqtt_user, mqtt_pwd) = (values.mqtt_host, values.mqtt_user, values.mqtt_pwd);

    let req = PayRequest {
        car_no,
        money,
        pay_type,
        park_no,
        server_ip,
        dsn,
        channel_name: String::new(),
        in_id: String::new(),
        auto_out,
    };

    println!("============================================================");
    println!("  正在下发测试支付...");
    println!("  车牌号码: {}", req.car_no);
    println!("  支付金额: {} 元", req.money);
    println!("  支付类型: {}", req.pay_type);
    println!("  车场编号: {}", req.park_no);
    println!("  目标 IP:  {}", req.server_ip);
    println!("  MQTT:     {}", mqtt_host);
    println!("============================================================");

    match execute_payment(&req, &mqtt_host, &mqtt_user, &mqtt_pwd) {
        Ok(res) => {
            println!("✅ 支付结果下发成功！");
            println!("  流水单号: {}", res.order_num);
            println!("  商户单号: {}", res.out_trade_no);
            println!("  支付时间: {}", res.pay_time);
            println!("  方式名称: {}", res.pay_type_name);
            if let Some(out_msg) = res.auto_out_result {
                println!("  出场联动: {}", out_msg);
            }
        }
        Err(e) => {
            eprintln!("❌ 支付下发失败: {}", e);
            std::process::exit(1);
        }
    }
}

/// 打印本次使用的远程探测凭据 (不含密码，便于排查连不上的原因)
fn print_remote_access(server_ip: &str) {
    let r = remote::current();
    println!("============================================================");
    println!("  🔑 远程探测凭据:");
    println!("     SSH   : {}@{}:{}", r.ssh_user, if server_ip.is_empty() { "<待选择车场>" } else { server_ip }, r.ssh_port);
    println!("     MySQL : {}@本机 {}", r.db_user, r.db_name);
    println!("     执行方式: {}", remote::transport_name());

    let mut builtin = Vec::new();
    if r.ssh_pwd_is_builtin() {
        builtin.push("SSH");
    }
    if r.db_pwd_is_builtin() {
        builtin.push("数据库");
    }
    if !builtin.is_empty() {
        println!("     ⚠️ {}密码仍为内置默认值，请在界面顶部「SSH / 数据库」中修改", builtin.join(" 与 "));
    }

    println!("     配置文件: {}", remote::config_path().display());
    println!("     修改方式: 界面顶部「SSH」「数据库」，或命令行/环境变量覆盖");
    println!("============================================================");
}

fn print_help() {
    println!(r#"
道尔云 5.0 - 电子支付模拟与下发工具 (Rust 版)

模式 1: HTTP API 服务模式 (推荐，供各种不同车场 IP 动态调用)
  drzk-pay-sim                   (默认启动在 0.0.0.0:18766，车场由界面「连接车场」决定)
  drzk-pay-sim --serve --port 8080 --server 192.168.65.59

  HTTP 调用示例 (Postman / cURL / 任何脚本):
  curl -X POST http://127.0.0.1:18766/api/pay \
       -H "Content-Type: application/json" \
       -d '{{"car_no":"粤B88801", "money":10.0, "server_ip":"192.168.65.58", "pay_type":57}}'
  注: 不传 server_ip / park_no 时，使用当前已探测成功的车场。

模式 2: CLI 命令行单次执行模式
  drzk-pay-sim --car <车牌> --server <车场IP> [选项]

命令行选项:
  -c, --car <车牌>         车牌号码 (必填，例如: 粤B88801)
  -m, --money <金额>       支付金额 (元)，默认 10.0
  -t, --pay-type <类型>    支付类型编码: 57=聚合V6, 1=微信, 2=支付宝, 0=现金 (默认 57)
      --server <IP>        目标车场服务器 IP (必填，车场编号与支付凭证均从该车场实时探测)
      --park-no <编号>     车场编号，默认采用探测到的真实编号 (一般无需指定)
      --dsn <DSN>          出口通道序列号 (仅出口扫码开闸需填写)
      --auto-out           自动联动目标车场出场放行接口
  -s, --serve              启动 HTTP 接口监听服务
  -p, --port <端口>        HTTP 服务端口，默认 18766
      --no-open            启动服务模式时不自动打开浏览器

远程探测凭据 (车场编号/MQTT 凭证/岗亭信息均通过 SSH + MySQL 实时探测):
      --ssh-user <用户>    SSH 登录用户，默认 root
      --ssh-pwd <密码>     SSH 登录密码，默认 root
      --ssh-port <端口>    SSH 端口，默认 22
      --db-user <用户>     MySQL 用户，默认 root
      --db-pwd <密码>      MySQL 密码，默认 123456
      --db-name <库名>     MySQL 库名，默认 ykt
  亦可使用环境变量: DRZK_SSH_USER / DRZK_SSH_PWD / DRZK_SSH_PORT /
                    DRZK_DB_USER / DRZK_DB_PWD / DRZK_DB_NAME
  (环境变量优先级低于命令行参数；探测失败会直接报错，不会回退到任何写死的车场编号)

  -h, --help               显示本帮助信息
"#);
}
