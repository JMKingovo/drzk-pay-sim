use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub struct MqttClient {
    stream: TcpStream,
    packet_id: u16,
}

impl MqttClient {
    /// 连接到指定的 MQTT Broker (例如 "121.37.253.10:1883")
    pub fn connect<A: ToSocketAddrs>(
        addr: A,
        client_id: &str,
        username: &str,
        password: &str,
        timeout: Duration,
    ) -> Result<Self, String> {
        let stream = TcpStream::connect(addr).map_err(|e| format!("连接 MQTT Broker 失败: {}", e))?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();

        let mut client = MqttClient {
            stream,
            packet_id: 1,
        };

        client.send_connect(client_id, username, password)?;
        client.read_connack()?;
        Ok(client)
    }

    /// 发送 MQTT 3.1.1 CONNECT 报文
    fn send_connect(&mut self, client_id: &str, user: &str, pass: &str) -> Result<(), String> {
        // Variable Header: Protocol Name ("MQTT"), Level 4 (3.1.1), Flags, Keepalive 60
        let mut var_header = vec![
            0x00, 0x04, b'M', b'Q', b'T', b'T',
            0x04,       // MQTT 3.1.1
            0xC2,       // User + Pass + CleanSession
            0x00, 0x3C, // KeepAlive 60s
        ];

        let mut payload = Vec::new();
        append_utf8(&mut payload, client_id);
        append_utf8(&mut payload, user);
        append_utf8(&mut payload, pass);

        let mut packet = vec![0x10]; // CONNECT
        append_remaining_length(&mut packet, var_header.len() + payload.len());
        packet.append(&mut var_header);
        packet.append(&mut payload);

        self.stream
            .write_all(&packet)
            .map_err(|e| format!("发送 CONNECT 失败: {}", e))
    }

    /// 读取 CONNACK 报文确认连接
    fn read_connack(&mut self) -> Result<(), String> {
        let mut buf = [0u8; 4];
        self.stream
            .read_exact(&mut buf)
            .map_err(|e| format!("读取 CONNACK 失败: {}", e))?;

        if buf[0] != 0x20 || buf[1] != 0x02 {
            return Err(format!("非法 CONNACK 响应: {:02X?}", buf));
        }

        match buf[3] {
            0x00 => Ok(()),
            0x01 => Err("连接被拒绝: 不支持的协议版本".into()),
            0x02 => Err("连接被拒绝: 标识符无效".into()),
            0x03 => Err("连接被拒绝: 服务不可用".into()),
            0x04 => Err("连接被拒绝: 错误的用户名或密码".into()),
            0x05 => Err("连接被拒绝: 未授权".into()),
            c => Err(format!("连接被拒绝: 错误码 0x{:02X}", c)),
        }
    }

    /// 发布消息 (QoS 1，带 PUBACK 确认)
    pub fn publish(&mut self, topic: &str, message: &str) -> Result<u16, String> {
        let pid = self.packet_id;
        self.packet_id = self.packet_id.wrapping_add(1);
        if self.packet_id == 0 {
            self.packet_id = 1;
        }

        let mut var_header = Vec::new();
        append_utf8(&mut var_header, topic);
        var_header.push((pid >> 8) as u8);
        var_header.push((pid & 0xFF) as u8);

        let payload = message.as_bytes();

        let mut packet = vec![0x32]; // PUBLISH (QoS 1)
        append_remaining_length(&mut packet, var_header.len() + payload.len());
        packet.append(&mut var_header);
        packet.extend_from_slice(payload);

        self.stream
            .write_all(&packet)
            .map_err(|e| format!("发送 PUBLISH 报文失败: {}", e))?;

        // 读取 PUBACK (4 bytes: 0x40, 0x02, pid_msb, pid_lsb)
        let mut ack = [0u8; 4];
        self.stream
            .read_exact(&mut ack)
            .map_err(|e| format!("等待 PUBACK 确认超时: {}", e))?;

        if ack[0] != 0x40 {
            return Err(format!("非法 PUBACK 响应: {:02X?}", ack));
        }

        Ok(pid)
    }

    /// 断开连接
    pub fn disconnect(mut self) {
        let _ = self.stream.write_all(&[0xE0, 0x00]);
    }
}

fn append_utf8(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len() as u16;
    buf.push((len >> 8) as u8);
    buf.push((len & 0xFF) as u8);
    buf.extend_from_slice(bytes);
}

fn append_remaining_length(buf: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 128;
        }
        buf.push(byte);
        if len == 0 {
            break;
        }
    }
}
