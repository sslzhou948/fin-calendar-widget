//! 主源：金十经济日历 WebSocket（`wss://wss-flash-2.jin10.com/`）。
//!
//! 协议依据用户提供的《金十经济日历 WebSocket 接口与长期运行指南 v5》
//! + 2026-10-23 POC 实测（`~/workspace/fin-calendar-poc/poc_jin10_results.md`）：
//! - 连接后收 16 字节 hello，派生 XOR key = `{B}.{A}`（A=int32LE@4，B=int32LE@8）
//! - 握手后所有帧整体 XOR；字符串 = int16 长度 + UTF-8；全小端
//! - 4002 visitor 登录（status=101）→ 4007 语音角色 echo → 2007 按日请求
//!   → 2006 响应（按 responseType + requestId 匹配）→ payload.data
//!   base64 + zlib → JSON 数组
//! - 1201 心跳回空二进制帧
//!
//! 本实现为**短连接**：每次 `fetch_week` 连上、拉整周（7 天 × subtype 0/2）、
//! 断开。请求量远低于官方客户端（其周同步节流下限 3 秒），POC 5 连全稳。
//!
//! 字段映射（POC 全字段清单已核对）：
//! - subtype 0（经济指标）：`indicator_name` / `country`（中文名）/ `star`
//!   原生星级 / `pub_time_unix` / `consensus` 预期 / `previous` 前值 /
//!   `actual` 实际值（原生！未公布为 null）/ `unit` 单位
//! - subtype 2（经济事件）：`event_content` / `country` / `star` /
//!   `event_time_unix`
//! - subtype 3（假期）结构不同（`{data,total,history}`），v1 不取，
//!   假期行由备源 Faireconomy 覆盖
//!
//! 时间：`pub_time` 即北京时间，`*_unix` 为绝对时间戳——直接转
//! Asia/Shanghai，**无需任何时区换算**（美东夏冬令时金十服务端已处理）。

use std::collections::HashSet;
use std::io::Read;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, NaiveDate};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use crate::mapping::classify_kind;
use crate::model::{CalendarEvent, Country, EventKind, Source};
use crate::schedule::TZ;
use crate::source::{DataSource, SourceError};

const ENDPOINT: &str = "wss://wss-flash-2.jin10.com/";
/// 整周拉取总超时（POC 实测整周约 1.6s，30s 留足余量）
const SESSION_TIMEOUT: Duration = Duration::from_secs(30);

const MSG_HEARTBEAT: i16 = 1201;
const MSG_LOGIN: i16 = 4002;
const MSG_VOICE: i16 = 4007;
const MSG_CAL_REQUEST: i16 = 2007;
const MSG_CAL_RESPONSE: i16 = 2006;

const SUBTYPE_INDICATORS: i32 = 0;
const SUBTYPE_EVENTS: i32 = 2;

// ---------------------------------------------------------------- XOR / 帧

/// 握手后全帧 XOR（与 POC 实现逐字一致）。
fn xor(data: &[u8], key: &str) -> Vec<u8> {
    let kb = key.as_bytes();
    if kb.is_empty() {
        return data.to_vec();
    }
    let first = kb[0] as usize;
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ kb[(i + first) % kb.len()])
        .collect()
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    out.extend_from_slice(&(b.len() as i16).to_le_bytes());
    out.extend_from_slice(b);
}

fn frame_login(machine_id: &str) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&MSG_LOGIN.to_le_bytes());
    f.extend_from_slice(&0i32.to_le_bytes()); // userId
    push_str(&mut f, ""); // token
    push_str(&mut f, machine_id);
    f.extend_from_slice(&0i32.to_le_bytes()); // userType
    push_str(&mut f, "win"); // platform
    push_str(&mut f, "0"); // language
    f
}

fn frame_voice() -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&MSG_VOICE.to_le_bytes());
    push_str(&mut f, "xiaochen");
    f
}

fn frame_calendar(subtype: i32, date: NaiveDate) -> Vec<u8> {
    let date_str = date.format("%Y-%m-%d").to_string();
    let date_int: i32 = date.format("%Y%m%d").to_string().parse().unwrap_or(0);
    let mut f = Vec::new();
    f.extend_from_slice(&MSG_CAL_REQUEST.to_le_bytes());
    f.extend_from_slice(&subtype.to_le_bytes());
    push_str(&mut f, &date_str);
    f.extend_from_slice(&date_int.to_le_bytes());
    f
}

/// 帧字段读取器（带边界检查，畸形帧返回 Parse 错误而非 panic）。
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], SourceError> {
        // checked_add 防御：不可信长度字段可使 pos + n 溢出回绕、绕过边界检查
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| SourceError::Parse("金十帧长度不足".into()))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn int16(&mut self) -> Result<i16, SourceError> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn int32(&mut self) -> Result<i32, SourceError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, SourceError> {
        let n = self.int16()?;
        if n < 0 {
            return Err(SourceError::Parse(format!(
                "金十帧字符串长度为负: {n}"
            )));
        }
        let bytes = self.take(n as usize)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

// ---------------------------------------------------------------- 解码/映射

/// 2006 payload 解码：JSON → `data` → base64 → zlib → JSON 数组。
fn decode_payload(payload: &str) -> Result<Vec<Value>, SourceError> {
    let outer: Value =
        serde_json::from_str(payload).map_err(|e| SourceError::Parse(format!("payload JSON: {e}")))?;
    let b64 = outer
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| SourceError::Parse("payload 缺 data 字段".into()))?;
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| SourceError::Parse(format!("base64: {e}")))?;
    // 解压输出上限 16MB：防损坏/恶意 payload 的解压炸弹（正常单日 JSON 远小于此；超限必解析失败走 Parse）
    const MAX_DECOMPRESSED: u64 = 16 * 1024 * 1024;
    let mut decoder = flate2::read::ZlibDecoder::new(&compressed[..]);
    let mut text = String::new();
    decoder
        .by_ref()
        .take(MAX_DECOMPRESSED)
        .read_to_string(&mut text)
        .map_err(|e| SourceError::Parse(format!("zlib: {e}")))?;
    let items: Value =
        serde_json::from_str(&text).map_err(|e| SourceError::Parse(format!("inner JSON: {e}")))?;
    match items {
        Value::Array(a) => Ok(a),
        _ => Err(SourceError::Parse("inner JSON 不是数组".into())),
    }
}

/// 中文国家名 → 内部国家枚举（POC 本周 20 个取值全覆盖）。
fn map_country(name: &str) -> Country {
    match name.trim() {
        "美国" => Country::US,
        "中国" => Country::CN,
        "欧元区" | "德国" | "法国" | "意大利" | "西班牙" => Country::EU,
        "英国" => Country::UK,
        "日本" => Country::JP,
        "澳大利亚" => Country::AU,
        "新西兰" => Country::NZ,
        "加拿大" => Country::CA,
        "瑞士" => Country::CH,
        _ => Country::Other,
    }
}

/// 宽容取字符串：字符串去空白（空→None），数字转字符串，其余→None。
fn clean_str(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// 数值拼单位：仅 `%` 拼接。
///
/// 其余单位（万人/亿美元…）不拼：内部宽容解析器 `parse_number` 是全串
/// 解析，拼上中文单位会让预期差着色整体退化为中性。同一数据源内三值
/// 单位一致，裸数字自洽。
fn with_unit(value: Option<String>, unit: Option<&str>) -> Option<String> {
    let v = value?;
    match unit {
        Some("%") => Some(format!("{v}%")),
        _ => Some(v),
    }
}

fn unix_to_sh(unix: i64) -> Option<DateTime<chrono_tz::Tz>> {
    DateTime::from_timestamp(unix, 0).map(|utc| utc.with_timezone(&TZ))
}

/// 发布时间：优先 `*_unix`，回退解析 `"yyyy-MM-dd HH:mm"` 字符串（北京时间）。
fn release_time(item: &Value, unix_key: &str, str_key: &str) -> Option<DateTime<chrono_tz::Tz>> {
    if let Some(u) = item.get(unix_key).and_then(Value::as_i64) {
        if u > 0 {
            if let Some(t) = unix_to_sh(u) {
                return Some(t);
            }
        }
    }
    let s = clean_str(item.get(str_key))?;
    let naive = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M").ok()?;
    use chrono::TimeZone;
    TZ.from_local_datetime(&naive).single()
}

/// 星级宽容解析：整数 / 浮点 / 字符串都认，识别失败回退 1
/// （服务端字段类型一旦变化，不让整行静默消失在过滤器后）。
fn parse_star(item: &Value) -> i64 {
    match item.get("star") {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(1),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(1),
        _ => 1,
    }
}

fn map_indicator(item: &Value) -> Option<CalendarEvent> {
    let name = clean_str(item.get("indicator_name"))?;
    let period = clean_str(item.get("time_period"));
    let title = match &period {
        Some(p) if !name.starts_with(p.as_str()) => format!("{p}{name}"),
        _ => name,
    };
    let release_at = release_time(item, "pub_time_unix", "pub_time")?;
    let unit = clean_str(item.get("unit"));
    let forecast = with_unit(clean_str(item.get("consensus")), unit.as_deref());
    let previous = with_unit(clean_str(item.get("previous")), unit.as_deref());
    let actual = with_unit(clean_str(item.get("actual")), unit.as_deref());
    let star = parse_star(item);
    let data_id = item.get("data_id").and_then(Value::as_i64).unwrap_or(0);
    let kind = classify_kind(
        &title,
        forecast.as_deref(),
        previous.as_deref(),
        actual.as_deref(),
    );
    Some(CalendarEvent {
        id: format!("jin10-d{data_id}"),
        release_at,
        country: map_country(&clean_str(item.get("country")).unwrap_or_default()),
        title,
        importance: star.clamp(1, 5) as u8,
        forecast,
        previous,
        actual,
        kind,
        status: None,
        source: Source::Jin10,
    })
}

fn map_event(item: &Value) -> Option<CalendarEvent> {
    let title = clean_str(item.get("event_content"))?;
    let release_at = release_time(item, "event_time_unix", "event_time")?;
    let star = parse_star(item);
    let id = item.get("id").and_then(Value::as_i64).unwrap_or(0);
    Some(CalendarEvent {
        id: format!("jin10-e{id}"),
        release_at,
        country: map_country(&clean_str(item.get("country")).unwrap_or_default()),
        title,
        importance: star.clamp(1, 5) as u8,
        forecast: None,
        previous: None,
        actual: None,
        kind: EventKind::Event,
        status: None,
        source: Source::Jin10,
    })
}

// ---------------------------------------------------------------- 数据源

pub struct Jin10;

impl Jin10 {
    pub fn new() -> Result<Self, SourceError> {
        Ok(Self)
    }

    /// 拉取指定日期集合并映射为事件（排序，不做空校验——
    /// 空校验是整周拉取的语义，单日探活允许 0 条）。
    async fn fetch_dates(
        &self,
        dates: &[NaiveDate],
    ) -> Result<Vec<CalendarEvent>, SourceError> {
        let items = tokio::time::timeout(SESSION_TIMEOUT, fetch_week_items(dates))
            .await
            .map_err(|_| SourceError::Network("金十拉取超时（30s）".into()))??;
        let mut events: Vec<CalendarEvent> = Vec::with_capacity(items.len());
        for (subtype, item) in &items {
            let ev = if *subtype == SUBTYPE_INDICATORS {
                map_indicator(item)
            } else {
                map_event(item)
            };
            if let Some(ev) = ev {
                events.push(ev);
            }
        }
        events.sort_by_key(|e| e.release_at);
        Ok(events)
    }

    /// 轻量探活（接口测试用，清单 #10）：完整握手/登录，但只拉一天
    /// （2 个请求），返回当日条数。会话能跑通协议即证明连通，
    /// 避免每次点测试都全量拉整周、徒增平台风控风险。
    pub async fn probe(&self, date: NaiveDate) -> Result<usize, SourceError> {
        Ok(self.fetch_dates(&[date]).await?.len())
    }
}

/// 每次连接生成一个 machineId（POC 验证随机值可行；官方客户端为持久
/// 值，长期稳定性差异属文档列明的未闭环项，先按实测走）。
fn machine_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:032x}", nanos ^ (std::process::id() as u128) << 64)
}

#[async_trait]
impl DataSource for Jin10 {
    fn name(&self) -> &'static str {
        "jin10-ws"
    }

    async fn fetch_week(&self, week_start: NaiveDate) -> Result<Vec<CalendarEvent>, SourceError> {
        let dates: Vec<NaiveDate> = (0..7)
            .map(|i| week_start + chrono::Duration::days(i))
            .collect();
        let events = self.fetch_dates(&dates).await?;
        if events.is_empty() {
            return Err(SourceError::Empty("金十整周返回 0 条".into()));
        }
        Ok(events)
    }
}

/// 一次短连接会话：握手 → 登录 → 4007 → 14 个按日请求 → 集齐 2006。
/// 返回 (subtype, item) 列表。任一日解码失败即整体失败（触发降级），
/// 避免静默缺日。
async fn fetch_week_items(
    dates: &[NaiveDate],
) -> Result<Vec<(i32, Value)>, SourceError> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(ENDPOINT)
        .await
        .map_err(|e| SourceError::Network(format!("金十 WS 连接失败: {e}")))?;

    // hello：首个 16 字节二进制帧（明文）
    let hello = next_binary(&mut ws).await?;
    if hello.len() != 16 {
        return Err(SourceError::Parse(format!(
            "hello 长度异常: {}",
            hello.len()
        )));
    }
    let a = i32::from_le_bytes(hello[4..8].try_into().unwrap());
    let b = i32::from_le_bytes(hello[8..12].try_into().unwrap());
    let key = format!("{b}.{a}");

    // 登录
    ws.send(Message::Binary(xor(&frame_login(&machine_id()), &key).into()))
        .await
        .map_err(|e| SourceError::Network(format!("发送登录帧失败: {e}")))?;
    wait_login(&mut ws, &key).await?;

    // 4007 语音角色（与日历无强依赖，但官方路径如此，实测最稳）
    ws.send(Message::Binary(xor(&frame_voice(), &key).into()))
        .await
        .map_err(|e| SourceError::Network(format!("发送 4007 失败: {e}")))?;
    wait_voice(&mut ws, &key).await?;

    // 按日请求 subtype 0 + 2
    let mut pending: HashSet<(i32, i32)> = HashSet::new();
    for subtype in [SUBTYPE_INDICATORS, SUBTYPE_EVENTS] {
        for d in dates {
            let date_int: i32 = d.format("%Y%m%d").to_string().parse().unwrap_or(0);
            pending.insert((subtype, date_int));
            ws.send(Message::Binary(xor(&frame_calendar(subtype, *d), &key).into()))
                .await
                .map_err(|e| SourceError::Network(format!("发送日历请求失败: {e}")))?;
        }
    }

    // 集齐响应
    let mut out: Vec<(i32, Value)> = Vec::new();
    while !pending.is_empty() {
        let raw = next_binary(&mut ws).await?;
        let plain = xor(&raw, &key);
        let mut r = Reader::new(&plain);
        let msg_id = r.int16()?;
        match msg_id {
            MSG_HEARTBEAT => {
                ws.send(Message::Binary(Vec::new().into()))
                    .await
                    .map_err(|e| SourceError::Network(format!("心跳回复失败: {e}")))?;
            }
            MSG_CAL_RESPONSE => {
                let resp_type = r.int32()?;
                let _date = r.string()?;
                let req_id = r.int32()?;
                let payload = r.string()?;
                if pending.remove(&(resp_type, req_id)) {
                    for item in decode_payload(&payload)? {
                        out.push((resp_type, item));
                    }
                }
            }
            _ => {} // 1200/1005 等服务端推送：忽略
        }
    }
    let _ = ws.close(None).await;
    Ok(out)
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 下一个二进制帧（文本/控制帧跳过；流结束 → Network 错误）。
async fn next_binary(ws: &mut WsStream) -> Result<Vec<u8>, SourceError> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(b))) => return Ok(b.to_vec()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(SourceError::Network(format!("金十 WS 接收失败: {e}"))),
            None => return Err(SourceError::Network("金十 WS 连接被关闭".into())),
        }
    }
}

async fn wait_login(ws: &mut WsStream, key: &str) -> Result<(), SourceError> {
    loop {
        let raw = next_binary(ws).await?;
        let plain = xor(&raw, key);
        let mut r = Reader::new(&plain);
        let msg_id = r.int16()?;
        if msg_id == MSG_HEARTBEAT {
            ws.send(Message::Binary(Vec::new().into()))
                .await
                .map_err(|e| SourceError::Network(format!("心跳回复失败: {e}")))?;
            continue;
        }
        if msg_id != MSG_LOGIN {
            continue;
        }
        // 响应体可能带字符串长度前缀，也可能裸 JSON：两种都试
        let body = &plain[2..];
        let text = Reader::new(body)
            .string()
            .ok()
            .filter(|s| s.contains("\"status\""))
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
        let start = text
            .find('{')
            .ok_or_else(|| SourceError::Parse("登录响应无 JSON".into()))?;
        let obj: Value = serde_json::from_str(&text[start..])
            .map_err(|e| SourceError::Parse(format!("登录响应 JSON: {e}")))?;
        if obj.get("status").and_then(Value::as_i64) == Some(101) {
            return Ok(());
        }
        return Err(SourceError::Network(format!(
            "金十 visitor 登录失败: {obj}"
        )));
    }
}

async fn wait_voice(ws: &mut WsStream, key: &str) -> Result<(), SourceError> {
    loop {
        let raw = next_binary(ws).await?;
        let plain = xor(&raw, key);
        let mut r = Reader::new(&plain);
        let msg_id = r.int16()?;
        if msg_id == MSG_HEARTBEAT {
            ws.send(Message::Binary(Vec::new().into()))
                .await
                .map_err(|e| SourceError::Network(format!("心跳回复失败: {e}")))?;
            continue;
        }
        if msg_id != MSG_VOICE {
            continue;
        }
        let echo = r.string()?;
        if echo == "xiaochen" {
            return Ok(());
        }
        return Err(SourceError::Parse(format!("4007 echo 异常: {echo}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_roundtrip_matches_poc() {
        // POC 算法：out[i] = data[i] ^ kb[(i + kb[0]) % len]
        let key = "123.456";
        let data: Vec<u8> = (0..64).collect();
        let enc = xor(&data, key);
        assert_ne!(enc, data);
        assert_eq!(xor(&enc, key), data);
        // 手算首字节：kb[0]='1'=49, (0+49)%7=0 → kb[0]=49
        assert_eq!(enc[0], 0 ^ 49);
    }

    #[test]
    fn calendar_frame_matches_doc_example() {
        // 文档示例：2026-10-23 subtype 0 →
        // d707 00000000 0a00 "2026-10-23" 9f283501
        let f = frame_calendar(0, NaiveDate::from_ymd_opt(2026, 10, 23).unwrap());
        let hex: String = f.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "d707000000000a00323032362d31302d32339f283501");
    }

    #[test]
    fn decode_payload_roundtrip() {
        let arr = r#"[{"data_id":1,"indicator_name":"测试"}]"#;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        enc.write_all(arr.as_bytes()).unwrap();
        let compressed = enc.finish().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);
        let payload = format!(r#"{{"data":"{b64}"}}"#);
        let items = decode_payload(&payload).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["indicator_name"], "测试");
    }

    #[test]
    fn reader_rejects_negative_string_length() {
        // 长度字段 -1（0xFFFF）：不得 panic、不得回绕切片
        let buf = [0xFFu8, 0xFF, 0x41, 0x42];
        let mut r = Reader::new(&buf);
        assert!(r.string().is_err());
        // pos=2 后再来负长度同样拒绝（长度前缀为小端：0x0002 = 2）
        let buf2 = [0x02u8, 0x00, b'a', b'b', 0xFF, 0xFF];
        let mut r2 = Reader::new(&buf2);
        assert_eq!(r2.string().unwrap(), "ab");
        assert!(r2.string().is_err());
    }

    #[test]
    fn star_parsing_is_tolerant() {
        let v: Value = serde_json::json!({"star": 4});
        assert_eq!(parse_star(&v), 4);
        let v: Value = serde_json::json!({"star": 4.0});
        assert_eq!(parse_star(&v), 4);
        let v: Value = serde_json::json!({"star": "5"});
        assert_eq!(parse_star(&v), 5);
        let v: Value = serde_json::json!({"star": null});
        assert_eq!(parse_star(&v), 1);
        let v: Value = serde_json::json!({});
        assert_eq!(parse_star(&v), 1);
    }

    #[test]
    fn country_mapping() {
        assert_eq!(map_country("美国"), Country::US);
        assert_eq!(map_country("中国"), Country::CN);
        assert_eq!(map_country("欧元区"), Country::EU);
        assert_eq!(map_country("德国"), Country::EU);
        assert_eq!(map_country("法国"), Country::EU);
        assert_eq!(map_country("英国"), Country::UK);
        assert_eq!(map_country("日本"), Country::JP);
        assert_eq!(map_country("澳大利亚"), Country::AU);
        assert_eq!(map_country("新西兰"), Country::NZ);
        assert_eq!(map_country("加拿大"), Country::CA);
        assert_eq!(map_country("瑞士"), Country::CH);
        assert_eq!(map_country("韩国"), Country::Other);
        assert_eq!(map_country(""), Country::Other);
    }

    #[test]
    fn value_cleaning_and_unit() {
        assert_eq!(clean_str(Some(&Value::String(" 4.5 ".into()))), Some("4.5".into()));
        assert_eq!(clean_str(Some(&Value::String("".into()))), None);
        assert_eq!(clean_str(Some(&Value::Null)), None);
        assert_eq!(clean_str(None), None);
        assert_eq!(with_unit(Some("4.5".into()), Some("%")), Some("4.5%".into()));
        // 非 % 单位不拼（保 parse_number 着色）
        assert_eq!(with_unit(Some("21.9".into()), Some("万人")), Some("21.9".into()));
        assert_eq!(with_unit(None, Some("%")), None);
    }

    #[test]
    fn map_indicator_sample() {
        // 依 POC 真实样本构造（中国 GDP 年率，2026-10-20 公布）
        let item: Value = serde_json::json!({
            "data_id": 1145001,
            "indicator_name": "GDP年率",
            "time_period": "第三季度",
            "country": "中国",
            "star": 4,
            "previous": "4.30",
            "consensus": "4.40",
            "actual": "4.5",
            "revised": null,
            "unit": "%",
            "pub_time": "2026-10-20 10:00",
            "pub_time_unix": 1792461600
        });
        let ev = map_indicator(&item).unwrap();
        assert_eq!(ev.id, "jin10-d1145001");
        assert_eq!(ev.title, "第三季度GDP年率");
        assert_eq!(ev.country, Country::CN);
        assert_eq!(ev.importance, 4);
        assert_eq!(ev.forecast.as_deref(), Some("4.40%"));
        assert_eq!(ev.previous.as_deref(), Some("4.30%"));
        assert_eq!(ev.actual.as_deref(), Some("4.5%"));
        assert_eq!(ev.kind, EventKind::Data);
        assert_eq!(ev.source, Source::Jin10);
        // 1792461600 = 2026-10-20 10:00 北京时间
        assert_eq!(ev.release_at.format("%Y-%m-%d %H:%M").to_string(), "2026-10-20 10:00");
    }

    #[test]
    fn map_indicator_unpublished_has_no_actual() {
        let item: Value = serde_json::json!({
            "data_id": 2,
            "indicator_name": "CPI年率",
            "country": "美国",
            "star": 5,
            "previous": "3.0",
            "consensus": "3.1",
            "actual": null,
            "unit": "%",
            "pub_time_unix": 1792461600
        });
        let ev = map_indicator(&item).unwrap();
        assert!(ev.actual.is_none());
        assert_eq!(ev.importance, 5);
    }

    #[test]
    fn map_event_sample() {
        let item: Value = serde_json::json!({
            "id": 1188475,
            "event_content": "澳洲联储主席布洛克发表讲话。",
            "country": "澳大利亚",
            "star": 3,
            "event_time": "2026-10-23 00:00",
            "event_time_unix": 1792684800
        });
        let ev = map_event(&item).unwrap();
        assert_eq!(ev.id, "jin10-e1188475");
        assert_eq!(ev.country, Country::AU);
        assert_eq!(ev.kind, EventKind::Event);
        assert_eq!(ev.importance, 3);
    }
}
