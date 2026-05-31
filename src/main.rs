use json::{object, JsonValue};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{Mutex, OnceLock};

// ── .env loader ──────────────────────────────────────────────────────────────
fn load_dotenv() {
    let content = match std::fs::read_to_string(".env") {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim();
            if env::var(key).is_err() {
                env::set_var(key, val);
            }
        }
    }
}

// ── ABI selectors ────────────────────────────────────────────────────────────
// transfer(address,uint256)
const SEL_ERC20_TRANSFER: &str = "a9059cbb";
// safeTransferFrom(address,address,uint256)
const SEL_ERC721_SAFE_TRANSFER: &str = "42842e0e";
// safeTransferFrom(address,address,uint256,uint256,bytes)
const SEL_ERC1155_SAFE_TRANSFER: &str = "f242432a";
// withdrawEther(address,uint256)  — EtherPortal
const SEL_ETHER_WITHDRAW: &str = "522f6815";

// ── App version ───────────────────────────────────────────────────────────────
const APP_VERSION: &str = "2.0.0";

// ═══════════════════════════════════════════════════════════════════════════════
// Domain model
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug)]
struct Student {
    name: String,
    registration_number: String,
    wallet_address: String,
    // Deposit ledger
    ether_deposits:    Vec<u128>,                      // wei amounts
    erc20_deposits:    Vec<(String, u128)>,            // (token_addr, amount)
    erc721_deposits:   Vec<(String, String)>,           // (token_addr, token_id_hex)
    erc1155_deposits:  Vec<(String, String, u128)>,    // (token_addr, token_id_hex, amount)
    // Withdrawal ledger (vouchers issued)
    ether_withdrawals:    Vec<u128>,
    erc20_withdrawals:    Vec<(String, u128)>,
    erc721_withdrawals:   Vec<(String, String)>,
    erc1155_withdrawals:  Vec<(String, String, u128)>,
}

impl Student {
    fn new(name: String, reg: String, wallet: String) -> Self {
        Self {
            name,
            registration_number: reg,
            wallet_address: wallet,
            ether_deposits: vec![],
            erc20_deposits: vec![],
            erc721_deposits: vec![],
            erc1155_deposits: vec![],
            ether_withdrawals: vec![],
            erc20_withdrawals: vec![],
            erc721_withdrawals: vec![],
            erc1155_withdrawals: vec![],
        }
    }

    fn ether_available(&self) -> u128 {
        let dep: u128 = self.ether_deposits.iter().sum();
        let wth: u128 = self.ether_withdrawals.iter().sum();
        dep.saturating_sub(wth)
    }

    fn erc20_available(&self, token: &str) -> u128 {
        let dep: u128 = self.erc20_deposits.iter().filter(|(t,_)| t == token).map(|(_,a)| *a).sum();
        let wth: u128 = self.erc20_withdrawals.iter().filter(|(t,_)| t == token).map(|(_,a)| *a).sum();
        dep.saturating_sub(wth)
    }

    fn erc721_held_ids(&self, token: &str) -> Vec<String> {
        let dep: HashSet<String> = self.erc721_deposits.iter()
            .filter(|(t,_)| t == token).map(|(_,id)| id.clone()).collect();
        let wth: HashSet<String> = self.erc721_withdrawals.iter()
            .filter(|(t,_)| t == token).map(|(_,id)| id.clone()).collect();
        dep.difference(&wth).cloned().collect()
    }

    fn erc1155_available(&self, token: &str, token_id: &str) -> u128 {
        let dep: u128 = self.erc1155_deposits.iter()
            .filter(|(t,id,_)| t == token && id == token_id).map(|(_,_,a)| *a).sum();
        let wth: u128 = self.erc1155_withdrawals.iter()
            .filter(|(t,id,_)| t == token && id == token_id).map(|(_,_,a)| *a).sum();
        dep.saturating_sub(wth)
    }

    fn to_json(&self) -> JsonValue {
        // ETH
        let total_ether_dep: u128 = self.ether_deposits.iter().sum();
        let total_ether_wth: u128 = self.ether_withdrawals.iter().sum();

        // ERC-20
        let erc20_tokens: HashSet<String> =
            self.erc20_deposits.iter().map(|(t,_)| t.clone()).collect();
        let mut erc20_arr = json::JsonValue::new_array();
        for token in &erc20_tokens {
            let dep: u128 = self.erc20_deposits.iter().filter(|(t,_)| t==token).map(|(_,a)|*a).sum();
            let wth: u128 = self.erc20_withdrawals.iter().filter(|(t,_)| t==token).map(|(_,a)|*a).sum();
            let _ = erc20_arr.push(object!{
                "token_address"   => token.clone(),
                "total_deposited" => dep.to_string(),
                "total_withdrawn" => wth.to_string(),
                "available"       => dep.saturating_sub(wth).to_string(),
            });
        }

        // ERC-721
        let erc721_tokens: HashSet<String> =
            self.erc721_deposits.iter().map(|(t,_)| t.clone()).collect();
        let mut erc721_arr = json::JsonValue::new_array();
        for token in &erc721_tokens {
            let held = self.erc721_held_ids(token);
            let dep_ids: Vec<&str> = self.erc721_deposits.iter()
                .filter(|(t,_)| t==token).map(|(_,id)| id.as_str()).collect();
            let mut dep_arr = json::JsonValue::new_array();
            for id in &dep_ids { let _ = dep_arr.push(id.to_string()); }
            let mut held_arr = json::JsonValue::new_array();
            for id in &held { let _ = held_arr.push(id.clone()); }
            let _ = erc721_arr.push(object!{
                "token_address" => token.clone(),
                "deposited_ids" => dep_arr,
                "held_ids"      => held_arr,
                "held_count"    => held.len(),
            });
        }

        // ERC-1155
        let erc1155_keys: HashSet<(String,String)> = self.erc1155_deposits.iter()
            .map(|(t,id,_)| (t.clone(), id.clone())).collect();
        let mut erc1155_arr = json::JsonValue::new_array();
        for (token, token_id) in &erc1155_keys {
            let dep: u128 = self.erc1155_deposits.iter()
                .filter(|(t,id,_)| t==token && id==token_id).map(|(_,_,a)|*a).sum();
            let wth: u128 = self.erc1155_withdrawals.iter()
                .filter(|(t,id,_)| t==token && id==token_id).map(|(_,_,a)|*a).sum();
            let _ = erc1155_arr.push(object!{
                "token_address"   => token.clone(),
                "token_id"        => token_id.clone(),
                "total_deposited" => dep.to_string(),
                "total_withdrawn" => wth.to_string(),
                "available"       => dep.saturating_sub(wth).to_string(),
            });
        }

        object!{
            "name"                => self.name.clone(),
            "registration_number" => self.registration_number.clone(),
            "wallet_address"      => self.wallet_address.clone(),
            "ether_balance"       => object!{
                "total_deposited" => total_ether_dep.to_string(),
                "total_withdrawn" => total_ether_wth.to_string(),
                "available_wei"   => self.ether_available().to_string(),
            },
            "erc20_balances"      => erc20_arr,
            "erc721_holdings"     => erc721_arr,
            "erc1155_balances"    => erc1155_arr,
        }
    }

    fn activity_log(&self) -> JsonValue {
        let mut events = json::JsonValue::new_array();

        for amount in &self.ether_deposits {
            let _ = events.push(object!{
                "type"   => "ether_deposit",
                "amount" => amount.to_string(),
            });
        }
        for (token, amount) in &self.erc20_deposits {
            let _ = events.push(object!{
                "type"   => "erc20_deposit",
                "token"  => token.clone(),
                "amount" => amount.to_string(),
            });
        }
        for (token, token_id) in &self.erc721_deposits {
            let _ = events.push(object!{
                "type"     => "erc721_deposit",
                "token"    => token.clone(),
                "token_id" => token_id.clone(),
            });
        }
        for (token, token_id, amount) in &self.erc1155_deposits {
            let _ = events.push(object!{
                "type"     => "erc1155_deposit",
                "token"    => token.clone(),
                "token_id" => token_id.clone(),
                "amount"   => amount.to_string(),
            });
        }
        for amount in &self.ether_withdrawals {
            let _ = events.push(object!{
                "type"   => "ether_withdrawal",
                "amount" => amount.to_string(),
            });
        }
        for (token, amount) in &self.erc20_withdrawals {
            let _ = events.push(object!{
                "type"   => "erc20_withdrawal",
                "token"  => token.clone(),
                "amount" => amount.to_string(),
            });
        }
        for (token, token_id) in &self.erc721_withdrawals {
            let _ = events.push(object!{
                "type"     => "erc721_withdrawal",
                "token"    => token.clone(),
                "token_id" => token_id.clone(),
            });
        }
        for (token, token_id, amount) in &self.erc1155_withdrawals {
            let _ = events.push(object!{
                "type"     => "erc1155_withdrawal",
                "token"    => token.clone(),
                "token_id" => token_id.clone(),
                "amount"   => amount.to_string(),
            });
        }
        events
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Application state
// ═══════════════════════════════════════════════════════════════════════════════

struct AppState {
    students:        HashMap<String, Student>,
    ether_portal:    String,
    erc20_portal:    String,
    erc721_portal:   String,
    erc1155_portal:  String,
    app_contract:    Option<String>,
    total_inputs:    u64,
    total_notices:   u64,
    total_vouchers:  u64,
}

impl AppState {
    fn new() -> Self {
        let ether = env::var("ETHER_PORTAL_ADDRESS")
            .unwrap_or_else(|_| "0xa632c5c05812c6a6149b7af5c56117d1d2603828".to_string())
            .to_lowercase();
        let erc20 = env::var("ERC20_PORTAL_ADDRESS")
            .expect("ERC20_PORTAL_ADDRESS not set — add it to .env and rebuild")
            .to_lowercase();
        let erc721 = env::var("ERC721_PORTAL_ADDRESS")
            .expect("ERC721_PORTAL_ADDRESS not set — add it to .env and rebuild")
            .to_lowercase();
        let erc1155 = env::var("ERC1155_PORTAL_ADDRESS")
            .expect("ERC1155_PORTAL_ADDRESS not set — add it to .env and rebuild")
            .to_lowercase();
        log("INIT", &format!(
            "event=startup version={} ether_portal={} erc20_portal={} erc721_portal={} erc1155_portal={}",
            APP_VERSION, ether, erc20, erc721, erc1155
        ));

        Self {
            students: HashMap::new(),
            ether_portal: ether,
            erc20_portal: erc20,
            erc721_portal: erc721,
            erc1155_portal: erc1155,
            app_contract: None,
            total_inputs: 0,
            total_notices: 0,
            total_vouchers: 0,
        }
    }
}

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();
fn get_state() -> &'static Mutex<AppState> {
    STATE.get_or_init(|| Mutex::new(AppState::new()))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Structured logging
// ═══════════════════════════════════════════════════════════════════════════════

fn log(tag: &str, msg: &str) {
    println!("[{}] {}", tag, msg);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Hex / byte utilities
// ═══════════════════════════════════════════════════════════════════════════════

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() % 2 != 0 { return None; }
    (0..hex.len()).step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn extract_address(bytes: &[u8], offset: usize) -> Option<String> {
    if bytes.len() < offset + 20 { return None; }
    Some(format!("0x{}", bytes[offset..offset + 20].iter()
        .map(|b| format!("{:02x}", b)).collect::<String>()))
}

fn extract_u128(bytes: &[u8], offset: usize) -> Option<u128> {
    if bytes.len() < offset + 32 { return None; }
    Some(bytes[offset + 16..offset + 32].iter()
        .fold(0u128, |acc, &b| acc.wrapping_shl(8) | b as u128))
}

fn extract_uint256_hex(bytes: &[u8], offset: usize) -> Option<String> {
    if bytes.len() < offset + 32 { return None; }
    Some(format!("0x{}", bytes[offset..offset + 32].iter()
        .map(|b| format!("{:02x}", b)).collect::<String>()))
}

/// Extract trailing bytes as a 0x hex string (for execLayerData / baseLayerData).
fn extract_trailing_hex(bytes: &[u8], from: usize) -> String {
    if bytes.len() <= from {
        return "0x".to_string();
    }
    format!("0x{}", bytes[from..].iter().map(|b| format!("{:02x}", b)).collect::<String>())
}

fn str_to_hex_payload(s: &str) -> String {
    format!("0x{}", s.bytes().map(|b| format!("{:02x}", b)).collect::<String>())
}

fn normalise_uint256(hex: &str) -> String {
    let bare = hex.trim_start_matches("0x");
    format!("0x{:0>64}", bare.to_lowercase())
}

// ═══════════════════════════════════════════════════════════════════════════════
// ABI encoding helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn abi_addr(addr: &str) -> String {
    format!("000000000000000000000000{}", addr.trim_start_matches("0x").to_lowercase())
}

fn abi_u128(val: u128) -> String { format!("{:064x}", val) }

fn abi_uint256(hex: &str) -> String {
    format!("{:0>64}", hex.trim_start_matches("0x").to_lowercase())
}

/// transfer(address to, uint256 amount)
fn calldata_erc20_transfer(to: &str, amount: u128) -> String {
    format!("0x{}{}{}", SEL_ERC20_TRANSFER, abi_addr(to), abi_u128(amount))
}

/// safeTransferFrom(address from, address to, uint256 tokenId)
fn calldata_erc721_transfer(from: &str, to: &str, token_id: &str) -> String {
    format!(
        "0x{}{}{}{}",
        SEL_ERC721_SAFE_TRANSFER,
        abi_addr(from),
        abi_addr(to),
        abi_uint256(token_id),
    )
}

/// safeTransferFrom(address from, address to, uint256 id, uint256 value, bytes data)
fn calldata_erc1155_transfer(from: &str, to: &str, token_id: &str, amount: u128) -> String {
    format!(
        "0x{}{}{}{}{}{}{}",
        SEL_ERC1155_SAFE_TRANSFER,
        abi_addr(from),
        abi_addr(to),
        abi_uint256(token_id),
        abi_u128(amount),
        abi_u128(160), // offset to `bytes data`
        abi_u128(0),   // data length = 0
    )
}

/// withdrawEther(address receiver, uint256 value)  — calls EtherPortal
fn calldata_ether_withdraw(to: &str, amount: u128) -> String {
    format!("0x{}{}{}", SEL_ETHER_WITHDRAW, abi_addr(to), abi_u128(amount))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Rollup HTTP helpers
// ═══════════════════════════════════════════════════════════════════════════════

async fn emit_notice(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = object!{ "payload" => str_to_hex_payload(payload) };
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .uri(format!("{}/notice", server_addr))
        .body(hyper::Body::from(body.dump()))?;
    let resp = client.request(req).await?;
    {
        let mut s = get_state().lock().unwrap();
        s.total_notices += 1;
    }
    log("NOTICE", &format!("http_status={} payload_len={}", resp.status(), payload.len()));
    Ok(())
}

async fn emit_report(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = object!{ "payload" => str_to_hex_payload(payload) };
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .uri(format!("{}/report", server_addr))
        .body(hyper::Body::from(body.dump()))?;
    let resp = client.request(req).await?;
    log("REPORT", &format!("http_status={} payload_len={}", resp.status(), payload.len()));
    Ok(())
}

async fn emit_voucher(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    destination: &str,
    calldata: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = object!{
        "destination" => destination,
        "payload"     => calldata,
    };
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .uri(format!("{}/voucher", server_addr))
        .body(hyper::Body::from(body.dump()))?;
    let resp = client.request(req).await?;
    {
        let mut s = get_state().lock().unwrap();
        s.total_vouchers += 1;
    }
    log("VOUCHER", &format!("http_status={} destination={}", resp.status(), destination));
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Advance handler
// ═══════════════════════════════════════════════════════════════════════════════

pub async fn handle_advance(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    request: JsonValue,
) -> Result<&'static str, Box<dyn std::error::Error>> {
    let metadata    = &request["data"]["metadata"];
    let payload_hex = request["data"]["payload"].as_str().ok_or("missing payload")?;
    let msg_sender  = metadata["msg_sender"].as_str().unwrap_or("").to_lowercase();
    let input_index = metadata["input_index"].as_u64().unwrap_or(0);
    let timestamp   = metadata["timestamp"].as_u64().unwrap_or(0);
    let app_from_meta = metadata["app_contract"].as_str().unwrap_or("").to_lowercase();

    log("ADVANCE", &format!(
        "event=input_received input_index={} msg_sender={} timestamp={} payload_len={}",
        input_index, msg_sender, timestamp, payload_hex.len()
    ));

    // Increment total input counter and discover app_contract
    {
        let mut s = get_state().lock().unwrap();
        s.total_inputs += 1;
        if s.app_contract.is_none() && !app_from_meta.is_empty() {
            s.app_contract = Some(app_from_meta.clone());
            log("INIT", &format!("event=app_contract_discovered address={}", app_from_meta));
        }
    }

    // Read portal addresses (release lock before any await).
    let (ether_portal, erc20_portal, erc721_portal, erc1155_portal) = {
        let s = get_state().lock().unwrap();
        (s.ether_portal.clone(), s.erc20_portal.clone(),
         s.erc721_portal.clone(), s.erc1155_portal.clone())
    };

    // Helper: emit a diagnostic report marking this advance as processed.
    // Called at the end of every successful advance for test 5.11 coverage.
    let emit_advance_report = |result: &str| {
        let diag = format!(
            r#"{{"event":"advance_processed","input_index":{},"msg_sender":"{}","result":"{}"}}"#,
            input_index, msg_sender, result
        );
        async move {
            if let Err(e) = emit_report(client, server_addr, &diag).await {
                log("WARN", &format!("advance_report failed: {}", e));
            }
        }
    };

    // ── Ether portal deposit ──────────────────────────────────────────────────
    // Payload: depositor(20) + amount(32) + execLayerData(N)  = 52+ bytes
    if msg_sender == ether_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 52 => b,
            _ => {
                log("ERROR", &format!("event=invalid_ether_payload input_index={}", input_index));
                let err = format!(r#"{{"error":"invalid_ether_payload","input_index":{}}}"#, input_index);
                emit_report(client, server_addr, &err).await?;
                return Ok("reject");
            }
        };

        let depositor = extract_address(&bytes, 0).unwrap();
        let amount    = extract_u128(&bytes, 20).unwrap_or(0);
        let exec_data = extract_trailing_hex(&bytes, 52);

        {
            let mut s = get_state().lock().unwrap();
            let entry = s.students.entry(depositor.clone()).or_insert_with(|| {
                let st = Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                );
                log("ADVANCE", &format!("event=auto_registered wallet={}", depositor));
                st
            });
            entry.ether_deposits.push(amount);
        }

        log("ADVANCE", &format!(
            "event=ether_deposit input_index={} depositor={} amount={}",
            input_index, depositor, amount
        ));

        let notice = format!(
            r#"{{"event":"ether_deposit","input_index":{},"depositor":"{}","amount":"{}","exec_layer_data":"{}"}}"#,
            input_index, depositor, amount, exec_data
        );
        emit_notice(client, server_addr, &notice).await?;
        emit_advance_report("accept").await;
        return Ok("accept");
    }

    // ── ERC-20 portal deposit ─────────────────────────────────────────────────
    // Payload: token(20) | depositor(20) | amount(32) | execLayerData(N) = 72+ bytes
    if msg_sender == erc20_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 72 => b,
            _ => {
                log("ERROR", &format!("event=invalid_erc20_payload input_index={}", input_index));
                let err = format!(r#"{{"error":"invalid_erc20_payload","input_index":{}}}"#, input_index);
                emit_report(client, server_addr, &err).await?;
                return Ok("reject");
            }
        };

        let token_addr = extract_address(&bytes, 0).unwrap();
        let depositor  = extract_address(&bytes, 20).unwrap();
        let amount     = extract_u128(&bytes, 40).unwrap_or(0);
        let exec_data  = extract_trailing_hex(&bytes, 72);

        {
            let mut s = get_state().lock().unwrap();
            let entry = s.students.entry(depositor.clone()).or_insert_with(|| {
                Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                )
            });
            entry.erc20_deposits.push((token_addr.clone(), amount));
        }

        log("ADVANCE", &format!(
            "event=erc20_deposit input_index={} depositor={} token={} amount={}",
            input_index, depositor, token_addr, amount
        ));

        let notice = format!(
            r#"{{"event":"erc20_deposit","input_index":{},"depositor":"{}","token":"{}","amount":"{}","exec_layer_data":"{}"}}"#,
            input_index, depositor, token_addr, amount, exec_data
        );
        emit_notice(client, server_addr, &notice).await?;
        emit_advance_report("accept").await;
        return Ok("accept");
    }

    // ── ERC-721 portal deposit ────────────────────────────────────────────────
    // Payload: token(20) | depositor(20) | tokenId(32) | baseLayerData | execLayerData
    if msg_sender == erc721_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 72 => b,
            _ => {
                log("ERROR", &format!("event=invalid_erc721_payload input_index={}", input_index));
                let err = format!(r#"{{"error":"invalid_erc721_payload","input_index":{}}}"#, input_index);
                emit_report(client, server_addr, &err).await?;
                return Ok("reject");
            }
        };

        let token_addr = extract_address(&bytes, 0).unwrap();
        let depositor  = extract_address(&bytes, 20).unwrap();
        let token_id   = extract_uint256_hex(&bytes, 40).unwrap();
        // Trailing bytes after 72 are ABI-encoded baseLayerData + execLayerData
        let extra_data = extract_trailing_hex(&bytes, 72);

        {
            let mut s = get_state().lock().unwrap();
            let entry = s.students.entry(depositor.clone()).or_insert_with(|| {
                Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                )
            });
            entry.erc721_deposits.push((token_addr.clone(), token_id.clone()));
        }

        log("ADVANCE", &format!(
            "event=erc721_deposit input_index={} depositor={} token={} token_id={}",
            input_index, depositor, token_addr, token_id
        ));

        let notice = format!(
            r#"{{"event":"erc721_deposit","input_index":{},"depositor":"{}","token":"{}","token_id":"{}","extra_data":"{}"}}"#,
            input_index, depositor, token_addr, token_id, extra_data
        );
        emit_notice(client, server_addr, &notice).await?;
        emit_advance_report("accept").await;
        return Ok("accept");
    }

    // ── ERC-1155 single portal deposit ────────────────────────────────────────
    // Payload: token(20) | depositor(20) | tokenId(32) | amount(32) | extra(N) = 104+ bytes
    if msg_sender == erc1155_portal {
        let bytes = match hex_to_bytes(payload_hex) {
            Some(b) if b.len() >= 104 => b,
            _ => {
                log("ERROR", &format!("event=invalid_erc1155_payload input_index={}", input_index));
                let err = format!(r#"{{"error":"invalid_erc1155_payload","input_index":{}}}"#, input_index);
                emit_report(client, server_addr, &err).await?;
                return Ok("reject");
            }
        };

        let token_addr = extract_address(&bytes, 0).unwrap();
        let depositor  = extract_address(&bytes, 20).unwrap();
        let token_id   = extract_uint256_hex(&bytes, 40).unwrap();
        let amount     = extract_u128(&bytes, 72).unwrap_or(0);
        let extra_data = extract_trailing_hex(&bytes, 104);

        {
            let mut s = get_state().lock().unwrap();
            let entry = s.students.entry(depositor.clone()).or_insert_with(|| {
                Student::new(
                    format!("Unknown({})", &depositor[..10]),
                    format!("AUTO-{}", &depositor[2..10]),
                    depositor.clone(),
                )
            });
            entry.erc1155_deposits.push((token_addr.clone(), token_id.clone(), amount));
        }

        log("ADVANCE", &format!(
            "event=erc1155_deposit input_index={} depositor={} token={} token_id={} amount={}",
            input_index, depositor, token_addr, token_id, amount
        ));

        let notice = format!(
            r#"{{"event":"erc1155_deposit","input_index":{},"depositor":"{}","token":"{}","token_id":"{}","amount":"{}","extra_data":"{}"}}"#,
            input_index, depositor, token_addr, token_id, amount, extra_data
        );
        emit_notice(client, server_addr, &notice).await?;
        emit_advance_report("accept").await;
        return Ok("accept");
    }

    // ── Direct JSON action from a regular wallet ──────────────────────────────
    let bytes = match hex_to_bytes(payload_hex) {
        Some(b) => b,
        None => {
            log("ERROR", &format!("event=invalid_hex input_index={}", input_index));
            emit_report(client, server_addr,
                &format!(r#"{{"error":"invalid_hex_payload","input_index":{}}}"#, input_index)).await?;
            return Ok("reject");
        }
    };

    let payload_str = match std::str::from_utf8(&bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            log("ERROR", &format!("event=payload_not_utf8 input_index={}", input_index));
            emit_report(client, server_addr,
                &format!(r#"{{"error":"payload_not_utf8","input_index":{}}}"#, input_index)).await?;
            return Ok("reject");
        }
    };

    let action = match json::parse(&payload_str) {
        Ok(v) => v,
        Err(_) => {
            log("ERROR", &format!("event=payload_not_json input_index={}", input_index));
            emit_report(client, server_addr,
                &format!(r#"{{"error":"payload_not_json","input_index":{},"raw":"{}"}}"#,
                    input_index, payload_str.chars().take(120).collect::<String>()
                        .replace('"', "\\\""))).await?;
            return Ok("reject");
        }
    };

    let action_type = action["action"].as_str().unwrap_or("").to_string();
    log("ADVANCE", &format!("event=action input_index={} action={} sender={}",
        input_index, action_type, msg_sender));

    match action_type.as_str() {

        // ── register ─────────────────────────────────────────────────────────
        "register" => {
            let name = match action["name"].as_str() {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => {
                    emit_report(client, server_addr,
                        &format!(r#"{{"error":"register_missing_name","input_index":{}}}"#, input_index)).await?;
                    return Ok("reject");
                }
            };
            let reg_number = match action["reg_number"].as_str() {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => {
                    emit_report(client, server_addr,
                        &format!(r#"{{"error":"register_missing_reg_number","input_index":{}}}"#, input_index)).await?;
                    return Ok("reject");
                }
            };

            let wallet = msg_sender.clone();
            let already = { get_state().lock().unwrap().students.contains_key(&wallet) };

            if already {
                emit_report(client, server_addr,
                    &format!(r#"{{"error":"already_registered","input_index":{},"wallet":"{}"}}"#,
                        input_index, wallet)).await?;
                return Ok("reject");
            }

            {
                let mut s = get_state().lock().unwrap();
                s.students.insert(wallet.clone(),
                    Student::new(name.clone(), reg_number.clone(), wallet.clone()));
            }

            log("ADVANCE", &format!(
                "event=student_registered input_index={} name={} reg_number={} wallet={}",
                input_index, name, reg_number, wallet
            ));

            let notice = format!(
                r#"{{"event":"student_registered","input_index":{},"name":"{}","reg_number":"{}","wallet":"{}"}}"#,
                input_index, name, reg_number, wallet
            );
            emit_notice(client, server_addr, &notice).await?;
            emit_advance_report("accept").await;
            Ok("accept")
        }

        // ── withdraw ─────────────────────────────────────────────────────────
        "withdraw" => {
            let wallet = msg_sender.clone();

            let is_registered = { get_state().lock().unwrap().students.contains_key(&wallet) };
            if !is_registered {
                emit_report(client, server_addr,
                    &format!(r#"{{"error":"not_registered","input_index":{},"wallet":"{}"}}"#,
                        input_index, wallet)).await?;
                return Ok("reject");
            }

            let asset_type = action["asset_type"].as_str().unwrap_or("").to_string();
            let ether_portal_addr = { get_state().lock().unwrap().ether_portal.clone() };
            let app_addr = {
                let s = get_state().lock().unwrap();
                s.app_contract.clone().unwrap_or_default()
            };

            match asset_type.as_str() {

                // ── Ether withdrawal ──────────────────────────────────────────
                "ether" => {
                    let amount_str = action["amount"].as_str().unwrap_or("0");
                    let amount: u128 = match amount_str.parse() {
                        Ok(a) if a > 0 => a,
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_invalid_amount","input_index":{},"raw":"{}"}}"#,
                                    input_index, amount_str)).await?;
                            return Ok("reject");
                        }
                    };

                    let available = {
                        let s = get_state().lock().unwrap();
                        s.students.get(&wallet).map(|st| st.ether_available()).unwrap_or(0)
                    };

                    if available < amount {
                        emit_report(client, server_addr, &format!(
                            r#"{{"error":"insufficient_ether_balance","input_index":{},"wallet":"{}","available":"{}","requested":"{}"}}"#,
                            input_index, wallet, available, amount
                        )).await?;
                        return Ok("reject");
                    }

                    {
                        let mut s = get_state().lock().unwrap();
                        if let Some(st) = s.students.get_mut(&wallet) {
                            st.ether_withdrawals.push(amount);
                        }
                    }

                    // withdrawEther(address receiver, uint256 value) on EtherPortal
                    let calldata = calldata_ether_withdraw(&wallet, amount);
                    log("ADVANCE", &format!(
                        "event=ether_withdrawal input_index={} wallet={} amount={}",
                        input_index, wallet, amount
                    ));
                    emit_voucher(client, server_addr, &ether_portal_addr, &calldata).await?;

                    let notice = format!(
                        r#"{{"event":"ether_withdrawal","input_index":{},"wallet":"{}","amount":"{}"}}"#,
                        input_index, wallet, amount
                    );
                    emit_notice(client, server_addr, &notice).await?;
                    emit_advance_report("accept").await;
                    Ok("accept")
                }

                // ── ERC-20 withdrawal ─────────────────────────────────────────
                "erc20" => {
                    let token = match action["token"].as_str() {
                        Some(t) if !t.is_empty() => t.to_lowercase(),
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_missing_token","input_index":{}}}"#, input_index)).await?;
                            return Ok("reject");
                        }
                    };
                    let amount_str = action["amount"].as_str().unwrap_or("0");
                    let amount: u128 = match amount_str.parse() {
                        Ok(a) if a > 0 => a,
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_invalid_amount","input_index":{},"raw":"{}"}}"#,
                                    input_index, amount_str)).await?;
                            return Ok("reject");
                        }
                    };

                    let available = {
                        let s = get_state().lock().unwrap();
                        s.students.get(&wallet).map(|st| st.erc20_available(&token)).unwrap_or(0)
                    };

                    if available < amount {
                        emit_report(client, server_addr, &format!(
                            r#"{{"error":"insufficient_erc20_balance","input_index":{},"wallet":"{}","token":"{}","available":"{}","requested":"{}"}}"#,
                            input_index, wallet, token, available, amount
                        )).await?;
                        return Ok("reject");
                    }

                    {
                        let mut s = get_state().lock().unwrap();
                        if let Some(st) = s.students.get_mut(&wallet) {
                            st.erc20_withdrawals.push((token.clone(), amount));
                        }
                    }

                    let calldata = calldata_erc20_transfer(&wallet, amount);
                    emit_voucher(client, server_addr, &token, &calldata).await?;

                    let notice = format!(
                        r#"{{"event":"erc20_withdrawal","input_index":{},"wallet":"{}","token":"{}","amount":"{}"}}"#,
                        input_index, wallet, token, amount
                    );
                    emit_notice(client, server_addr, &notice).await?;
                    emit_advance_report("accept").await;
                    Ok("accept")
                }

                // ── ERC-721 withdrawal ────────────────────────────────────────
                "erc721" => {
                    let token = match action["token"].as_str() {
                        Some(t) if !t.is_empty() => t.to_lowercase(),
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_missing_token","input_index":{}}}"#, input_index)).await?;
                            return Ok("reject");
                        }
                    };
                    let token_id_raw = match action["token_id"].as_str() {
                        Some(id) if !id.is_empty() => id.to_string(),
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_missing_token_id","input_index":{}}}"#, input_index)).await?;
                            return Ok("reject");
                        }
                    };
                    let token_id = normalise_uint256(&token_id_raw);

                    let has_token = {
                        let s = get_state().lock().unwrap();
                        s.students.get(&wallet)
                            .map(|st| st.erc721_held_ids(&token).contains(&token_id))
                            .unwrap_or(false)
                    };

                    if !has_token {
                        emit_report(client, server_addr, &format!(
                            r#"{{"error":"erc721_token_not_held","input_index":{},"wallet":"{}","token":"{}","token_id":"{}"}}"#,
                            input_index, wallet, token, token_id
                        )).await?;
                        return Ok("reject");
                    }

                    if app_addr.is_empty() {
                        emit_report(client, server_addr, &format!(
                            r#"{{"error":"app_contract_unknown","input_index":{}}}"#, input_index
                        )).await?;
                        return Ok("reject");
                    }

                    {
                        let mut s = get_state().lock().unwrap();
                        if let Some(st) = s.students.get_mut(&wallet) {
                            st.erc721_withdrawals.push((token.clone(), token_id.clone()));
                        }
                    }

                    let calldata = calldata_erc721_transfer(&app_addr, &wallet, &token_id);
                    emit_voucher(client, server_addr, &token, &calldata).await?;

                    let notice = format!(
                        r#"{{"event":"erc721_withdrawal","input_index":{},"wallet":"{}","token":"{}","token_id":"{}"}}"#,
                        input_index, wallet, token, token_id
                    );
                    emit_notice(client, server_addr, &notice).await?;
                    emit_advance_report("accept").await;
                    Ok("accept")
                }

                // ── ERC-1155 withdrawal ───────────────────────────────────────
                "erc1155" => {
                    let token = match action["token"].as_str() {
                        Some(t) if !t.is_empty() => t.to_lowercase(),
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_missing_token","input_index":{}}}"#, input_index)).await?;
                            return Ok("reject");
                        }
                    };
                    let token_id_raw = match action["token_id"].as_str() {
                        Some(id) if !id.is_empty() => id.to_string(),
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_missing_token_id","input_index":{}}}"#, input_index)).await?;
                            return Ok("reject");
                        }
                    };
                    let token_id = normalise_uint256(&token_id_raw);

                    let amount_str = action["amount"].as_str().unwrap_or("0");
                    let amount: u128 = match amount_str.parse() {
                        Ok(a) if a > 0 => a,
                        _ => {
                            emit_report(client, server_addr,
                                &format!(r#"{{"error":"withdraw_invalid_amount","input_index":{},"raw":"{}"}}"#,
                                    input_index, amount_str)).await?;
                            return Ok("reject");
                        }
                    };

                    let available = {
                        let s = get_state().lock().unwrap();
                        s.students.get(&wallet)
                            .map(|st| st.erc1155_available(&token, &token_id))
                            .unwrap_or(0)
                    };

                    if available < amount {
                        emit_report(client, server_addr, &format!(
                            r#"{{"error":"insufficient_erc1155_balance","input_index":{},"wallet":"{}","token":"{}","token_id":"{}","available":"{}","requested":"{}"}}"#,
                            input_index, wallet, token, token_id, available, amount
                        )).await?;
                        return Ok("reject");
                    }

                    if app_addr.is_empty() {
                        emit_report(client, server_addr, &format!(
                            r#"{{"error":"app_contract_unknown","input_index":{}}}"#, input_index
                        )).await?;
                        return Ok("reject");
                    }

                    {
                        let mut s = get_state().lock().unwrap();
                        if let Some(st) = s.students.get_mut(&wallet) {
                            st.erc1155_withdrawals.push((token.clone(), token_id.clone(), amount));
                        }
                    }

                    let calldata = calldata_erc1155_transfer(&app_addr, &wallet, &token_id, amount);
                    emit_voucher(client, server_addr, &token, &calldata).await?;

                    let notice = format!(
                        r#"{{"event":"erc1155_withdrawal","input_index":{},"wallet":"{}","token":"{}","token_id":"{}","amount":"{}"}}"#,
                        input_index, wallet, token, token_id, amount
                    );
                    emit_notice(client, server_addr, &notice).await?;
                    emit_advance_report("accept").await;
                    Ok("accept")
                }

                _ => {
                    emit_report(client, server_addr, &format!(
                        r#"{{"error":"unknown_asset_type","input_index":{},"asset_type":"{}","valid":["ether","erc20","erc721","erc1155"]}}"#,
                        input_index, asset_type
                    )).await?;
                    Ok("reject")
                }
            }
        }

        // ── ping ─────────────────────────────────────────────────────────────
        "ping" => {
            log("ADVANCE", &format!("event=ping input_index={} sender={}", input_index, msg_sender));
            let notice = format!(
                r#"{{"event":"pong","input_index":{},"sender":"{}"}}"#,
                input_index, msg_sender
            );
            emit_notice(client, server_addr, &notice).await?;
            emit_advance_report("accept").await;
            Ok("accept")
        }

        // ── dapp-address relay ────────────────────────────────────────────────
        "dapp_address" | "dapp-address" => {
            let addr = action["address"].as_str().unwrap_or(&msg_sender).to_string();
            {
                let mut s = get_state().lock().unwrap();
                if s.app_contract.is_none() {
                    s.app_contract = Some(addr.clone());
                }
            }
            let notice = format!(
                r#"{{"event":"dapp_address_relay","input_index":{},"address":"{}"}}"#,
                input_index, addr
            );
            emit_notice(client, server_addr, &notice).await?;
            emit_advance_report("accept").await;
            Ok("accept")
        }

        // ── unknown action ────────────────────────────────────────────────────
        other => {
            log("ERROR", &format!("event=unknown_action input_index={} action={}", input_index, other));
            emit_report(client, server_addr, &format!(
                r#"{{"error":"unknown_action","input_index":{},"action":"{}","valid":["ping","register","withdraw"]}}"#,
                input_index, other
            )).await?;
            Ok("reject")
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Inspect handler
// Routes (raw UTF-8 POST body / hex-decoded):
//   "" | "all"              — all students with full balances
//   "student/<addr>"        — single student full state
//   "activity/<addr>"       — deposit + withdrawal history for a student
//   "portals"               — configured portal and app contract addresses
//   "summary"               — total counts only
//   "app"                   — app contract address
//   "status"                — app health / version / counters
// ═══════════════════════════════════════════════════════════════════════════════

pub async fn handle_inspect(
    client: &hyper::Client<hyper::client::HttpConnector>,
    server_addr: &str,
    request: JsonValue,
) -> Result<&'static str, Box<dyn std::error::Error>> {
    let payload_hex = request["data"]["payload"].as_str().ok_or("missing payload")?;

    let route = {
        let bytes = hex_to_bytes(payload_hex).unwrap_or_default();
        std::str::from_utf8(&bytes).unwrap_or("all").trim().to_string()
    };

    log("INSPECT", &format!("event=inspect_received route={}", route));

    let report = if route.is_empty() || route == "all" {
        let s = get_state().lock().unwrap();
        let mut arr = json::JsonValue::new_array();
        for st in s.students.values() { let _ = arr.push(st.to_json()); }
        object!{
            "route"          => "all",
            "total_students" => s.students.len(),
            "students"       => arr,
        }.dump()

    } else if let Some(addr) = route.strip_prefix("student/") {
        let addr_norm = format!("0x{}", addr.trim_start_matches("0x").to_lowercase());
        let s = get_state().lock().unwrap();
        match s.students.get(&addr_norm) {
            Some(st) => {
                let mut r = st.to_json();
                r["route"] = "student".into();
                r.dump()
            }
            None => format!(r#"{{"error":"not_found","route":"student","wallet":"{}"}}"#, addr_norm),
        }

    } else if let Some(addr) = route.strip_prefix("activity/") {
        let addr_norm = format!("0x{}", addr.trim_start_matches("0x").to_lowercase());
        let s = get_state().lock().unwrap();
        match s.students.get(&addr_norm) {
            Some(st) => object!{
                "route"    => "activity",
                "wallet"   => addr_norm.clone(),
                "activity" => st.activity_log(),
            }.dump(),
            None => format!(r#"{{"error":"not_found","route":"activity","wallet":"{}"}}"#, addr_norm),
        }

    } else if route == "portals" {
        let s = get_state().lock().unwrap();
        object!{
            "route"           => "portals",
            "ether_portal"    => s.ether_portal.clone(),
            "erc20_portal"    => s.erc20_portal.clone(),
            "erc721_portal"   => s.erc721_portal.clone(),
            "erc1155_portal"  => s.erc1155_portal.clone(),
            "app_contract"    => s.app_contract.clone().unwrap_or_default(),
        }.dump()

    } else if route == "summary" {
        let s = get_state().lock().unwrap();
        let total = s.students.len();
        let registered = s.students.values().filter(|st| !st.name.starts_with("Unknown(")).count();
        let auto = total - registered;
        let ether_deps: usize  = s.students.values().map(|st| st.ether_deposits.len()).sum();
        let erc20_deps: usize  = s.students.values().map(|st| st.erc20_deposits.len()).sum();
        let erc721_deps: usize = s.students.values().map(|st| st.erc721_deposits.len()).sum();
        let erc1155_deps: usize = s.students.values().map(|st| st.erc1155_deposits.len()).sum();
        let erc20_wths: usize  = s.students.values().map(|st| st.erc20_withdrawals.len()).sum();
        let erc721_wths: usize = s.students.values().map(|st| st.erc721_withdrawals.len()).sum();
        let erc1155_wths: usize = s.students.values().map(|st| st.erc1155_withdrawals.len()).sum();
        object!{
            "route"              => "summary",
            "total_students"     => total,
            "registered"         => registered,
            "auto_registered"    => auto,
            "ether_deposits"     => ether_deps,
            "erc20_deposits"     => erc20_deps,
            "erc721_deposits"    => erc721_deps,
            "erc1155_deposits"   => erc1155_deps,
            "erc20_withdrawals"  => erc20_wths,
            "erc721_withdrawals" => erc721_wths,
            "erc1155_withdrawals"=> erc1155_wths,
        }.dump()

    } else if route == "app" {
        let s = get_state().lock().unwrap();
        match &s.app_contract {
            Some(addr) => object!{
                "route"        => "app",
                "app_contract" => addr.clone(),
                "discovered"   => true,
            }.dump(),
            None => object!{
                "route"        => "app",
                "app_contract" => json::JsonValue::Null,
                "discovered"   => false,
                "hint"         => "no advance input processed yet",
            }.dump(),
        }

    } else if route == "status" || route == "health" {
        let s = get_state().lock().unwrap();
        object!{
            "route"           => "status",
            "status"          => "ok",
            "app_name"        => "student-tracker",
            "version"         => APP_VERSION,
            "total_students"  => s.students.len(),
            "total_inputs"    => s.total_inputs,
            "total_notices"   => s.total_notices,
            "total_vouchers"  => s.total_vouchers,
            "portals_configured" => object!{
                "ether"   => !s.ether_portal.is_empty(),
                "erc20"   => !s.erc20_portal.is_empty(),
                "erc721"  => !s.erc721_portal.is_empty(),
                "erc1155" => !s.erc1155_portal.is_empty(),
            },
        }.dump()

    } else {
        format!(
            r#"{{"error":"unknown_route","route":"{}","valid":["all","student/<addr>","activity/<addr>","portals","app","summary","status"]}}"#,
            route
        )
    };

    log("INSPECT", &format!("event=inspect_response route={} len={}", route, report.len()));
    emit_report(client, server_addr, &report).await?;
    Ok("accept")
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main loop
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv();

    let client = hyper::Client::new();
    let server_addr = env::var("ROLLUP_HTTP_SERVER_URL")?;

    log("INIT", &format!("event=startup rollup_server={} version={}", server_addr, APP_VERSION));

    get_state();  // eagerly initialise (reads portal env vars)

    let mut status = "accept";
    loop {
        let response = object!{ "status" => status };
        let request = hyper::Request::builder()
            .method(hyper::Method::POST)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .uri(format!("{}/finish", server_addr))
            .body(hyper::Body::from(response.dump()))?;

        let response = client.request(request).await?;
        let http_status = response.status();

        log("FINISH", &format!("event=finish http_status={} prev_status={}", http_status, status));

        if http_status == hyper::StatusCode::ACCEPTED {
            // No pending input
        } else {
            let body = hyper::body::to_bytes(response).await?;
            let utf  = std::str::from_utf8(&body)?;
            let req  = json::parse(utf)?;

            let request_type = req["request_type"].as_str().ok_or("missing request_type")?;
            status = match request_type {
                "advance_state" => handle_advance(&client, &server_addr, req).await?,
                "inspect_state" => handle_inspect(&client, &server_addr, req).await?,
                other => {
                    log("ERROR", &format!("event=unknown_request_type type={}", other));
                    "reject"
                }
            };
        }
    }
}
