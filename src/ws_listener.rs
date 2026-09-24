use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use chrono::Local;

use crate::models::{DetectedPopup, ServerConfig};

pub type SharedPopup = Arc<Mutex<Option<DetectedPopup>>>;
pub type SharedConfig = Arc<Mutex<ServerConfig>>;

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

            if target_ip != last_ip {
                println!("🔄 检测到目标车场 IP 切换: {} -> {}", last_ip, target_ip);
                last_ip = target_ip.clone();
            }

            let addr = format!("{}:8089", target_ip);
            let path = format!("/box/boxControl/{}/{}", box_id, percent_encode(&user));

            match connect_ws(&addr, &path) {
                Ok(mut stream) => {
                    {
                        let mut c = cfg.lock().unwrap();
                        c.is_connected = true;
                        c.status_text = format!("已连接 {}:8089 (岗亭:{}, 车场:{})", target_ip, box_id, park_no);
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

/// 主动探测车场配置 (从 MySQL 查出真实 PARK_NUM 和在线岗亭)
pub fn probe_server_details(server_ip: &str) -> (String, String, String) {
    use std::process::Command;

    let mut park_no = "H51810900057".to_string();
    let mut box_id = "1".to_string();
    let mut user = "超级管理员".to_string();

    // 1. 查 PARK_NUM
    let sql_park = "PAGER=cat mysql -uroot -p123456 ykt -B -e \"SELECT parameter_value FROM sys_parameters WHERE parameter_code='PARK_NUM';\" 2>/dev/null";
    if let Ok(out) = Command::new("sshpass")
        .args(&["-p", "root", "ssh", "-o", "StrictHostKeyChecking=no", "-o", "ConnectTimeout=2", &format!("root@{}", server_ip), sql_park])
        .output()
    {
        let txt = String::from_utf8_lossy(&out.stdout);
        if let Some(val) = txt.lines().nth(1) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                park_no = trimmed.to_string();
            }
        }
    }

    // 2. 查当前在线岗亭与操作员
    let sql_booth = "PAGER=cat mysql -uroot -p123456 ykt -B -e \"SELECT box_id, loginName FROM park_local_set WHERE online=1 LIMIT 1;\" 2>/dev/null";
    if let Ok(out) = Command::new("sshpass")
        .args(&["-p", "root", "ssh", "-o", "StrictHostKeyChecking=no", "-o", "ConnectTimeout=2", &format!("root@{}", server_ip), sql_booth])
        .output()
    {
        let txt = String::from_utf8_lossy(&out.stdout);
        if let Some(val) = txt.lines().nth(1) {
            let parts: Vec<&str> = val.split('\t').collect();
            if parts.len() >= 2 {
                box_id = parts[0].trim().to_string();
                let u = parts[1].trim();
                if !u.is_empty() {
                    user = u.to_string();
                }
            }
        }
    }

    (park_no, box_id, user)
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
    let current_ip = { cfg.lock().unwrap().server_ip.clone() };

    loop {
        // 如果目标 IP 已经改变，立即断开当前 stream 退出重连
        {
            let now_ip = cfg.lock().unwrap().server_ip.clone();
            if now_ip != current_ip {
                println!("🔌 目标 IP 已变更，断开旧连接 [{}]", current_ip);
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
