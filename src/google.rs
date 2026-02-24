use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const SCOPES: &str = "https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/calendar.readonly";

const BUNDLED_CLIENT_ID: &str = "PLACEHOLDER.apps.googleusercontent.com";
const BUNDLED_CLIENT_SECRET: &str = "";

fn client_id() -> String {
    std::env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| BUNDLED_CLIENT_ID.to_string())
}

fn client_secret() -> String {
    std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| BUNDLED_CLIENT_SECRET.to_string())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoogleToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

impl GoogleToken {
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now().timestamp() >= self.expires_at - 60
    }
}

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub _id: String,
    pub title: String,
    pub start_date: String,
    pub end_date: String,
    pub calendar_name: String,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub all_day: bool,
}

#[derive(Debug, Clone)]
pub struct Email {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub date: String,
    pub content: Option<String>,
    pub is_read: bool,
}

fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join("google_token.json")
}

pub fn load_token(data_dir: &Path) -> Option<GoogleToken> {
    let content = std::fs::read_to_string(token_path(data_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_token(data_dir: &Path, token: &GoogleToken) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(token).map_err(|e| e.to_string())?;
    std::fs::write(token_path(data_dir), json).map_err(|e| e.to_string())
}

pub fn delete_token(data_dir: &Path) {
    let _ = std::fs::remove_file(token_path(data_dir));
}

pub fn get_valid_token(data_dir: &Path) -> Result<String, String> {
    let mut token = load_token(data_dir).ok_or("Not connected to Google")?;
    if token.is_expired() {
        token = refresh_access_token(&token.refresh_token)?;
        save_token(data_dir, &token)?;
    }
    Ok(token.access_token)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{:02X}", b));
            }
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

fn random_base64url(len: usize) -> String {
    let bytes: Vec<u8> = (0..len)
        .map(|_| uuid::Uuid::new_v4().as_bytes()[0])
        .collect();
    URL_SAFE_NO_PAD.encode(&bytes)
}

fn generate_code_verifier() -> String {
    let u1 = uuid::Uuid::new_v4();
    let u2 = uuid::Uuid::new_v4();
    let u3 = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 48];
    bytes[..16].copy_from_slice(u1.as_bytes());
    bytes[16..32].copy_from_slice(u2.as_bytes());
    bytes[32..].copy_from_slice(u3.as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn code_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

pub fn authorize(data_dir: &Path) -> Result<GoogleToken, String> {
    let verifier = generate_code_verifier();
    let challenge = code_challenge(&verifier);
    let state = random_base64url(16);

    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Failed to bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let cid = client_id();
    let auth_url = format!(
        "{AUTH_ENDPOINT}?response_type=code\
         &client_id={}\
         &redirect_uri={}\
         &scope={}\
         &state={}\
         &code_challenge={}\
         &code_challenge_method=S256\
         &access_type=offline\
         &prompt=consent",
        percent_encode(&cid),
        percent_encode(&redirect_uri),
        percent_encode(SCOPES),
        percent_encode(&state),
        percent_encode(&challenge),
    );

    let _ = std::process::Command::new("open").arg(&auth_url).spawn();

    let (mut stream, _) = listener
        .accept()
        .map_err(|e| format!("Failed to accept callback: {e}"))?;

    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
    let request = String::from_utf8_lossy(&buf[..n]).to_string();

    let html = "<html><body style=\"font-family:system-ui;text-align:center;padding:60px\">\
                <h1>Connected!</h1><p>You can close this tab and return to aipm.</p></body></html>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    drop(stream);

    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");
    let query = path.split('?').nth(1).unwrap_or("");
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    if let Some(error) = params.get("error") {
        return Err(format!("Authorization denied: {error}"));
    }

    let recv_state = params.get("state").ok_or("Missing state parameter")?;
    if *recv_state != state.as_str() {
        return Err("State mismatch".to_string());
    }

    let raw_code = params.get("code").ok_or("Missing authorization code")?;
    let auth_code = percent_decode(raw_code);

    let token = exchange_code(&auth_code, &verifier, &redirect_uri)?;
    save_token(data_dir, &token)?;
    Ok(token)
}

fn exchange_code(code: &str, verifier: &str, redirect_uri: &str) -> Result<GoogleToken, String> {
    let cid = client_id();
    let csec = client_secret();
    let resp = ureq::post(TOKEN_ENDPOINT)
        .send_form(&[
            ("code", code),
            ("client_id", &cid),
            ("client_secret", &csec),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
        ])
        .map_err(|e| format!("Token exchange failed: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    Ok(GoogleToken {
        access_token: json["access_token"]
            .as_str()
            .ok_or("Missing access_token")?
            .to_string(),
        refresh_token: json["refresh_token"]
            .as_str()
            .ok_or("Missing refresh_token")?
            .to_string(),
        expires_at: chrono::Utc::now().timestamp() + json["expires_in"].as_i64().unwrap_or(3600),
    })
}

fn refresh_access_token(refresh_token: &str) -> Result<GoogleToken, String> {
    let cid = client_id();
    let csec = client_secret();
    let resp = ureq::post(TOKEN_ENDPOINT)
        .send_form(&[
            ("refresh_token", refresh_token),
            ("client_id", &cid),
            ("client_secret", &csec),
            ("grant_type", "refresh_token"),
        ])
        .map_err(|e| format!("Token refresh failed: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    Ok(GoogleToken {
        access_token: json["access_token"]
            .as_str()
            .ok_or("Missing access_token")?
            .to_string(),
        refresh_token: refresh_token.to_string(),
        expires_at: chrono::Utc::now().timestamp() + json["expires_in"].as_i64().unwrap_or(3600),
    })
}

pub fn get_upcoming_events(token: &str, days: u32) -> Result<Vec<CalendarEvent>, String> {
    let now = chrono::Utc::now();
    let future = now + chrono::Duration::days(days as i64);

    let resp = ureq::get("https://www.googleapis.com/calendar/v3/calendars/primary/events")
        .set("Authorization", &format!("Bearer {token}"))
        .query("timeMin", &now.to_rfc3339())
        .query("timeMax", &future.to_rfc3339())
        .query("singleEvents", "true")
        .query("orderBy", "startTime")
        .query("maxResults", "100")
        .call()
        .map_err(|e| format!("Calendar API error: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let empty = Vec::new();
    let items = json["items"].as_array().unwrap_or(&empty);
    let cal_name = json["summary"].as_str().unwrap_or("primary").to_string();

    let events = items
        .iter()
        .map(|item| {
            let start = &item["start"];
            let end = &item["end"];
            let all_day = start["date"].is_string();
            CalendarEvent {
                _id: item["id"].as_str().unwrap_or("").to_string(),
                title: item["summary"].as_str().unwrap_or("(No title)").to_string(),
                start_date: start["dateTime"]
                    .as_str()
                    .or(start["date"].as_str())
                    .unwrap_or("")
                    .to_string(),
                end_date: end["dateTime"]
                    .as_str()
                    .or(end["date"].as_str())
                    .unwrap_or("")
                    .to_string(),
                calendar_name: cal_name.clone(),
                location: item["location"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                notes: item["description"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                all_day,
            }
        })
        .collect();

    Ok(events)
}

pub fn get_recent_emails(token: &str, limit: u32) -> Result<Vec<Email>, String> {
    let resp = ureq::get("https://gmail.googleapis.com/gmail/v1/users/me/messages")
        .set("Authorization", &format!("Bearer {token}"))
        .query("q", "is:unread in:inbox")
        .query("maxResults", &limit.to_string())
        .call()
        .map_err(|e| format!("Gmail list error: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let msg_refs = match json["messages"].as_array() {
        Some(m) => m,
        None => return Ok(Vec::new()),
    };

    let mut emails = Vec::new();
    for msg_ref in msg_refs {
        let msg_id = match msg_ref["id"].as_str() {
            Some(id) => id,
            None => continue,
        };

        let msg_resp = ureq::get(&format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{msg_id}"
        ))
        .set("Authorization", &format!("Bearer {token}"))
        .query("format", "full")
        .call();

        let msg_json: serde_json::Value = match msg_resp {
            Ok(r) => r.into_json().map_err(|e| e.to_string())?,
            Err(_) => continue,
        };

        let headers = msg_json["payload"]["headers"].as_array();
        let find_header = |name: &str| -> String {
            headers
                .and_then(|h| {
                    h.iter()
                        .find(|hdr| {
                            hdr["name"]
                                .as_str()
                                .map(|n| n.eq_ignore_ascii_case(name))
                                .unwrap_or(false)
                        })
                        .and_then(|hdr| hdr["value"].as_str())
                })
                .unwrap_or("")
                .to_string()
        };

        let snippet = msg_json["snippet"].as_str().unwrap_or("").to_string();
        let body = extract_body(&msg_json["payload"]).unwrap_or_else(|| snippet.clone());

        emails.push(Email {
            id: msg_id.to_string(),
            subject: find_header("Subject"),
            sender: find_header("From"),
            date: find_header("Date"),
            content: Some(if body.is_empty() { snippet } else { body }),
            is_read: false,
        });
    }

    Ok(emails)
}

fn extract_body(payload: &serde_json::Value) -> Option<String> {
    if let Some(data) = payload["body"]["data"].as_str() {
        if !data.is_empty() {
            return decode_base64url(data).ok();
        }
    }

    if let Some(parts) = payload["parts"].as_array() {
        for part in parts {
            if part["mimeType"].as_str() == Some("text/plain") {
                if let Some(data) = part["body"]["data"].as_str() {
                    return decode_base64url(data).ok();
                }
            }
        }
        for part in parts {
            if let Some(body) = extract_body(part) {
                return Some(body);
            }
        }
    }

    None
}

fn decode_base64url(data: &str) -> Result<String, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(data))
        .map_err(|e| format!("Base64 decode error: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("UTF-8 error: {e}"))
}
