#![recursion_limit = "256"]

mod http_server;
mod models;
mod mqtt;
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
        let mut server_ip = "192.168.65.58".to_string();
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

        let (park_no, box_id, user) = ws_listener::probe_server_details(&server_ip);
        let shared_config = Arc::new(Mutex::new(models::ServerConfig {
            server_ip: server_ip.clone(),
            park_no,
            box_id,
            login_name: user,
            is_connected: false,
            status_text: format!("正在连接 {}:8089...", server_ip),
        }));

        let shared_popup = Arc::new(Mutex::new(None));
        start_ws_listener(shared_config.clone(), shared_popup.clone());

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
        http_server::start_http_server(&addr, shared_config, shared_popup);
        return;
    }

    // 否则作为 CLI 命令行工具直接下发
    let mut car_no = String::new();
    let mut money = 10.0;
    let mut pay_type = 57;
    let mut server_ip = "192.168.65.58".to_string();
    let mut park_no = "H51810900057".to_string();
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
                    park_no = args[i + 1].clone();
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

    let req = PayRequest {
        car_no,
        money,
        pay_type,
        park_no,
        server_ip,
        dsn,
        in_id: String::new(),
        auto_out,
    };

    println!("============================================================");
    println!("  🚗 正在下发测试支付...");
    println!("  车牌号码: {}", req.car_no);
    println!("  支付金额: {} 元", req.money);
    println!("  支付类型: {}", req.pay_type);
    println!("  车场编号: {}", req.park_no);
    println!("  目标 IP:  {}", req.server_ip);
    println!("============================================================");

    match execute_payment(&req) {
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

fn print_help() {
    println!(r#"
道尔云 5.0 - 电子支付模拟与下发工具 (Rust 版)

模式 1: HTTP API 服务模式 (推荐，供各种不同车场 IP 动态调用)
  drzk-pay-sim                   (默认启动在 0.0.0.0:18766)
  drzk-pay-sim --serve --port 8080

  HTTP 调用示例 (Postman / cURL / 任何脚本):
  curl -X POST http://127.0.0.1:18766/api/pay \
       -H "Content-Type: application/json" \
       -d '{{"car_no":"粤B88801", "money":10.0, "server_ip":"192.168.65.58", "pay_type":57}}'

模式 2: CLI 命令行单次执行模式
  drzk-pay-sim --car <车牌> [选项]

命令行选项:
  -c, --car <车牌>         车牌号码 (必填，例如: 粤B88801)
  -m, --money <金额>       支付金额 (元)，默认 10.0
  -t, --pay-type <类型>    支付类型编码: 57=聚合V6, 1=微信, 2=支付宝, 0=现金 (默认 57)
      --server <IP>        目标车场服务器 IP，默认 192.168.65.58
      --park-no <编号>     车场编号，默认 H51810900057
      --dsn <DSN>          出口通道序列号 (仅出口扫码开闸需填写)
      --auto-out           自动联动目标车场出场放行接口
  -s, --serve              启动 HTTP 接口监听服务
  -p, --port <端口>        HTTP 服务端口，默认 18766
  -h, --help               显示本帮助信息
"#);
}
