use json::{object, JsonValue};
use std::collections::HashMap;
use std::env;
use std::sync::{Mutex, OnceLock};

// =============================================================================
// Portal addresses — v2.2.0 (CREATE2 deterministic, same on all EVM chains).
// Override via env vars ERC20_PORTAL_ADDRESS / ERC721_PORTAL_ADDRESS /
// ERC1155_PORTAL_ADDRESS if running against a devnet that differs.
// =============================================================================
const DEFAULT_ERC20_PORTAL: &str = "0xaca6586a0cf05bd831f2501e7b4aea550da6562d";
const DEFAULT_ERC721_PORTAL: &str = "0x9e8851dadb2b77103928518846c4678d48b5e371";
const DEFAULT_ERC1155_PORTAL: &str = "0x18558398dd1a8ce20956287a4da7b76ae7a96662";

// =============================================================================
// Domain model
// =============================================================================

#[derive(Clone, Debug)]
struct Student {
    name: String,
    registration_number: String,
    wallet_address: String,
    erc20_deposits: Vec<(String, u128)>,            // (token_addr, amount)
    erc721_deposits: Vec<(String, String)>,          // (token_addr, token_id_hex)
    erc1155_deposits: Vec<(String, String, u128)>,   // (token_addr, token_id_hex, amount)
}

impl Student {
    fn new(name: String, reg_number: String, wallet: String) -> Self {
        Self {
            name,
            registration_number: reg_number,
            wallet_address: wallet,
            erc20_deposits: Vec::new(),
            erc721_deposits: Vec::new(),
            erc1155_deposits: Vec::new(),
        }
    }

    fn to_json(&self) -> JsonValue {
        // Aggregate ERC20 by token address
        let mut erc20_totals: HashMap<String, u128> = HashMap::new();
        for (token, amount) in &self.erc20_deposits {
            *erc20_totals.entry(token.clone()).or_insert(0) += amount;
        }
        let mut erc20_arr = json::JsonValue::new_array();
        for (token, total) in &erc20_totals {
            let _ = erc20_arr.push(object! {
                "token_address" => token.clone(),
                "total_amount"  => total.to_string(),
                "deposit_count" => self.erc20_deposits.iter().filter(|(t, _)| t == token).count(),
            });
        }

        // ERC721 — list every deposited token
        let mut erc721_arr = json::JsonValue::new_array();
        for (token, token_id) in &self.erc721_deposits {
            let _ = erc721_arr.push(object! {
                "token_address" => token.clone(),
                "token_id"      => token_id.clone(),
            });
        }

        // Aggregate ERC1155 by (token_addr, token_id)
        let mut erc1155_totals: HashMap<(String, String), u128> = HashMap::new();
        for (token, token_id, amount) in &self.erc1155_deposits {
            *erc1155_totals
                .entry((token.clone(), token_id.clone()))
                .or_insert(0) += amount;
        }
        let mut erc1155_arr = json::JsonValue::new_array();
        for ((token, token_id), total) in &erc1155_totals {
            let _ = erc1155_arr.push(object! {
                "token_address" => token.clone(),
                "token_id"      => token_id.clone(),
                "total_amount"  => total.to_string(),
            });
        }

        object! {
            "name"                => self.name.clone(),
            "registration_number" => self.registration_number.clone(),
            "wallet_address"      => self.wallet_address.clone(),
            "erc20_deposits"      => erc20_arr,
            "erc721_deposits"     => erc721_arr,
            "erc1155_deposits"    => erc1155_arr,
        }
    }
}

// =============================================================================
// Application state
// =============================================================================

struct AppState {
    students: HashMap<String, Student>, // lowercase wallet_address -> Student
    erc20_portal: String,
    erc721_portal: String,
    erc1155_portal: String,
}

impl AppState {
    fn new() -> Self {
        let erc20_portal = env::var("ERC20_PORTAL_ADDRESS")
            .unwrap_or_else(|_| DEFAULT_ERC20_PORTAL.to_string())
            .to_lowercase();
        let erc721_portal = env::var("ERC721_PORTAL_ADDRESS")
            .unwrap_or_else(|_| DEFAULT_ERC721_PORTAL.to_string())
            .to_lowercase();
        let erc1155_portal = env::var("ERC1155_PORTAL_ADDRESS")
            .unwrap_or_else(|_| DEFAULT_ERC1155_PORTAL.to_string())
            .to_lowercase();

        println!("[student-tracker] Portal configuration:");
        println!("  ERC20  portal : {}", erc20_portal);
        println!("  ERC721 portal : {}", erc721_portal);
        println!("  ERC1155 portal: {}", erc1155_portal);

        Self {
            students: HashMap::new(),
            erc20_portal,
            erc721_portal,
            erc1155_portal,
        }
    }
}

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();

fn get_state() -> &'static Mutex<AppState> {
    STATE.get_or_init(|| Mutex::new(AppState::new()))
}

// =============================================================================
// Hex / byte utilities
// =============================================================================

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Extract a 20-byte Ethereum address from `bytes` at `offset`.
fn extract_address(bytes: &[u8], offset: usize) -> Option<String> {
    if bytes.len() < offset + 20 {
        return None;
    }
    let hex: String = bytes[offset..offset + 20]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    Some(format!("0x{}", hex))
}

/// Extract a uint256 from `bytes` at `offset` as u128 (truncates high bits for large values).
fn extract_u128(bytes: &[u8], offset: usize) -> Option<u128> {
    if bytes.len() < offset + 32 {
        return None;
    }
    // uint256 is big-endian 32 bytes; take the last 16 bytes for u128
    let val = bytes[offset + 16..offset + 32]
        .iter()
        .fold(0u128, |acc, &b| acc.wrapping_shl(8) | b as u128);
    Some(val)
}

/// Extract a uint256 from `bytes` at `offset` as a 0x-prefixed hex string (lossless).
fn extract_uint256_hex(bytes: &[u8], offset: usize) -> Option<String> {
    if bytes.len() < offset + 32 {
        return None;
    }
    let hex: String = bytes[offset..offset + 32]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    Some(format!("0x{}", hex))
}

/// Encode a UTF-8 string as a 0x-prefixed hex payload for notices/reports.
fn str_to_hex_payload(s: &str) -> String {
    let hex: String = s.bytes().map(|b| format!("{:02x}", b)).collect();
    format!("0x{}", hex)
}

// =============================================================================
// Rollup HTTP helpers  (do not hold a mutex lock when calling these)
// =============================================================================

async fn emit_notice(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = object! { "payload" => str_to_hex_payload(payload).as_str() };
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .uri(format!("{}/notice", server_addr))
        .body(hyper::Body::from(body.dump()))?;
    let resp = client.request(req).await?;
    println!("[notice] status={}", resp.status());
    Ok(())
}

async fn emit_report(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = object! { "payload" => str_to_hex_payload(payload).as_str() };
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .uri(format!("{}/report", server_addr))
        .body(hyper::Body::from(body.dump()))?;
    let resp = client.request(req).await?;
    println!("[report] status={}", resp.status());
    Ok(())
}

// =============================================================================
// Advance handler
// =============================================================================

pub async fn handle_advance(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    request: JsonValue,
) -> Result<&'static str, Box<dyn std::error::Error>> {
    println!("[advance] request={}", &request);

    let payload_hex = request["data"]["payload"]
        .as_str()
        .ok_or("Missing payload")?;
    let msg_sender = request["data"]["metadata"]["msg_sender"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();

    println!("[advance] msg_sender={}", msg_sender);

    // Read portal addresses without holding the lock across await points.
    let (erc20_portal, erc721_portal, erc1155_portal) = {
        let s = get_state().lock().unwrap();
        (s.erc20_portal.clone(), s.erc721_portal.clone(), s.erc1155_portal.clone())
    };

    // ------------------------------------------------------------------
    // ERC-20 portal deposit
    // Payload layout: token(20) | depositor(20) | amount(32)  = 72 bytes
    // ------------------------------------------------------------------
    if msg_sender == erc20_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 72 => b,
            _ => {
                emit_report(client, server_addr, r#"{"error":"invalid_erc20_payload"}"#).await?;
                return Ok("reject");
            }
        };

        let token_addr = extract_address(&bytes, 0).unwrap();
        let depositor   = extract_address(&bytes, 20).unwrap();
        let amount      = extract_u128(&bytes, 40).unwrap_or(0);

        {
            let mut state = get_state().lock().unwrap();
            let entry = state.students.entry(depositor.clone()).or_insert_with(|| {
                Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                )
            });
            entry.erc20_deposits.push((token_addr.clone(), amount));
        }

        let notice = format!(
            r#"{{"event":"erc20_deposit","depositor":"{}","token":"{}","amount":"{}"}}"#,
            depositor, token_addr, amount
        );
        emit_notice(client, server_addr, &notice).await?;
        println!("[advance] ERC20 depositor={} token={} amount={}", depositor, token_addr, amount);
        return Ok("accept");
    }

    // ------------------------------------------------------------------
    // ERC-721 portal deposit
    // Payload layout: token(20) | depositor(20) | tokenId(32)  = 72 bytes
    // ------------------------------------------------------------------
    if msg_sender == erc721_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 72 => b,
            _ => {
                emit_report(client, server_addr, r#"{"error":"invalid_erc721_payload"}"#).await?;
                return Ok("reject");
            }
        };

        let token_addr = extract_address(&bytes, 0).unwrap();
        let depositor   = extract_address(&bytes, 20).unwrap();
        let token_id    = extract_uint256_hex(&bytes, 40).unwrap();

        {
            let mut state = get_state().lock().unwrap();
            let entry = state.students.entry(depositor.clone()).or_insert_with(|| {
                Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                )
            });
            entry.erc721_deposits.push((token_addr.clone(), token_id.clone()));
        }

        let notice = format!(
            r#"{{"event":"erc721_deposit","depositor":"{}","token":"{}","token_id":"{}"}}"#,
            depositor, token_addr, token_id
        );
        emit_notice(client, server_addr, &notice).await?;
        println!("[advance] ERC721 depositor={} token={} tokenId={}", depositor, token_addr, token_id);
        return Ok("accept");
    }

    // ------------------------------------------------------------------
    // ERC-1155 single portal deposit
    // Payload layout: token(20) | depositor(20) | tokenId(32) | amount(32) = 104 bytes
    // ------------------------------------------------------------------
    if msg_sender == erc1155_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 104 => b,
            _ => {
                emit_report(client, server_addr, r#"{"error":"invalid_erc1155_payload"}"#).await?;
                return Ok("reject");
            }
        };

        let token_addr = extract_address(&bytes, 0).unwrap();
        let depositor   = extract_address(&bytes, 20).unwrap();
        let token_id    = extract_uint256_hex(&bytes, 40).unwrap();
        let amount      = extract_u128(&bytes, 72).unwrap_or(0);

        {
            let mut state = get_state().lock().unwrap();
            let entry = state.students.entry(depositor.clone()).or_insert_with(|| {
                Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                )
            });
            entry.erc1155_deposits.push((token_addr.clone(), token_id.clone(), amount));
        }

        let notice = format!(
            r#"{{"event":"erc1155_deposit","depositor":"{}","token":"{}","token_id":"{}","amount":"{}"}}"#,
            depositor, token_addr, token_id, amount
        );
        emit_notice(client, server_addr, &notice).await?;
        println!("[advance] ERC1155 depositor={} token={} tokenId={} amount={}", depositor, token_addr, token_id, amount);
        return Ok("accept");
    }

    // ------------------------------------------------------------------
    // Direct JSON action from a regular wallet
    // Expected: {"action":"register","name":"...","reg_number":"..."}
    // ------------------------------------------------------------------
    let bytes = match hex_to_bytes(payload_hex) {
        Some(b) => b,
        None => {
            emit_report(client, server_addr, r#"{"error":"invalid_hex_payload"}"#).await?;
            return Ok("reject");
        }
    };

    let payload_str = match std::str::from_utf8(&bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            emit_report(client, server_addr, r#"{"error":"payload_not_utf8"}"#).await?;
            return Ok("reject");
        }
    };

    let action = match json::parse(&payload_str) {
        Ok(v) => v,
        Err(_) => {
            emit_report(client, server_addr, r#"{"error":"payload_not_json"}"#).await?;
            return Ok("reject");
        }
    };

    match action["action"].as_str().unwrap_or("") {
        "register" => {
            let name = match action["name"].as_str() {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => {
                    emit_report(client, server_addr, r#"{"error":"register_missing_name"}"#).await?;
                    return Ok("reject");
                }
            };
            let reg_number = match action["reg_number"].as_str() {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => {
                    emit_report(client, server_addr, r#"{"error":"register_missing_reg_number"}"#).await?;
                    return Ok("reject");
                }
            };

            let wallet = msg_sender.clone();

            let already_exists = {
                get_state().lock().unwrap().students.contains_key(&wallet)
            };
            if already_exists {
                let err = format!(
                    r#"{{"error":"already_registered","wallet":"{}"}}"#,
                    wallet
                );
                emit_report(client, server_addr, &err).await?;
                return Ok("reject");
            }

            {
                let mut state = get_state().lock().unwrap();
                state.students.insert(
                    wallet.clone(),
                    Student::new(name.clone(), reg_number.clone(), wallet.clone()),
                );
            }

            let notice = format!(
                r#"{{"event":"student_registered","name":"{}","reg_number":"{}","wallet":"{}"}}"#,
                name, reg_number, wallet
            );
            emit_notice(client, server_addr, &notice).await?;
            println!("[advance] Registered: name={} reg={} wallet={}", name, reg_number, wallet);
            Ok("accept")
        }
        other => {
            let err = format!(r#"{{"error":"unknown_action","action":"{}"}}"#, other);
            emit_report(client, server_addr, &err).await?;
            Ok("reject")
        }
    }
}

// =============================================================================
// Inspect handler
// Routes:
//   "all"              — list every student
//   "student/<addr>"   — fetch one student by wallet address
//   "portals"          — show configured portal addresses
// =============================================================================

pub async fn handle_inspect(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    request: JsonValue,
) -> Result<&'static str, Box<dyn std::error::Error>> {
    println!("[inspect] request={}", &request);

    let payload_hex = request["data"]["payload"]
        .as_str()
        .ok_or("Missing payload")?;

    let route = {
        let bytes = hex_to_bytes(payload_hex).unwrap_or_default();
        std::str::from_utf8(&bytes)
            .unwrap_or("all")
            .trim()
            .to_string()
    };

    println!("[inspect] route={}", route);

    let report = if route.is_empty() || route == "all" {
        let state = get_state().lock().unwrap();
        let mut arr = json::JsonValue::new_array();
        for student in state.students.values() {
            let _ = arr.push(student.to_json());
        }
        let result = object! {
            "total_students" => state.students.len(),
            "students"       => arr,
        };
        result.dump()
    } else if let Some(addr) = route.strip_prefix("student/") {
        let addr_lower = format!(
            "0x{}",
            addr.trim_start_matches("0x").to_lowercase()
        );
        let state = get_state().lock().unwrap();
        match state.students.get(&addr_lower) {
            Some(s) => s.to_json().dump(),
            None => format!(r#"{{"error":"not_found","wallet":"{}"}}"#, addr_lower),
        }
    } else if route == "portals" {
        let state = get_state().lock().unwrap();
        object! {
            "erc20_portal"   => state.erc20_portal.clone(),
            "erc721_portal"  => state.erc721_portal.clone(),
            "erc1155_portal" => state.erc1155_portal.clone(),
        }
        .dump()
    } else {
        format!(r#"{{"error":"unknown_route","route":"{}"}}"#, route)
    };

    emit_report(client, server_addr, &report).await?;
    Ok("accept")
}

// =============================================================================
// Main — finish loop
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = hyper::Client::new();
    let server_addr = env::var("ROLLUP_HTTP_SERVER_URL")?;

    println!("[student-tracker] Starting student registry...");
    // Eagerly initialize state (reads portal env vars)
    get_state();

    let mut status = "accept";
    loop {
        println!("Sending finish");
        let response = object! {"status" => status};
        let request = hyper::Request::builder()
            .method(hyper::Method::POST)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .uri(format!("{}/finish", &server_addr))
            .body(hyper::Body::from(response.dump()))?;
        let response = client.request(request).await?;
        println!("Received finish status {}", response.status());

        if response.status() == hyper::StatusCode::ACCEPTED {
            println!("No pending rollup request, trying again");
        } else {
            let body = hyper::body::to_bytes(response).await?;
            let utf = std::str::from_utf8(&body)?;
            let req = json::parse(utf)?;

            let request_type = req["request_type"]
                .as_str()
                .ok_or("request_type is not a string")?;
            status = match request_type {
                "advance_state" => handle_advance(&client, &server_addr[..], req).await?,
                "inspect_state" => handle_inspect(&client, &server_addr[..], req).await?,
                _ => {
                    eprintln!("Unknown request type");
                    "reject"
                }
            };
        }
    }
}
