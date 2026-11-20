//! Аттестация сборки против 0.4.7 (`tools/reference/online/attestation.js` →
//! `fixtures/attestation.json`): запись ключа, подписи Ed25519 (байты совпадают с Node — подпись
//! детерминирована), шаблон `X-HLSR-Build`, `BuildAttestation` против того же двойника сервера,
//! что в скрипте (HMAC-вызов и билет, счётчик вместо случайных чисел), с поддельными часами.
//! Случаи самотеста 0.4.7 (`attestation-selftest.js:80-237`) входят в сценарии фикстуры.

mod support;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use hlsr_core::json_text::{json_parse, stringify};
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::providers::BoxFuture;
use hlsr_online::security::attest_seal;
use hlsr_online::security::attestation::{
    AttestationOptions, BuildAttestation, valid_build_header,
};
use hlsr_online::security::build_identity::BuildIdentity;
use hlsr_online::security::build_key::{AttestKey, BuildKey, NoKey};
use hlsr_online::telemetry::transport::{
    Headers, RequestOptions, Response, Transport, TransportError,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use support::{FakeClock, Mismatches, fixture, items, log_lines, str_of};

fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `seedOf(n)` скрипта: `sha256("seed-<n>")`.
fn seed_of(n: usize) -> [u8; 32] {
    Sha256::digest(format!("seed-{n}").as_bytes()).into()
}

fn public_of(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// Запись ключа скрипта: `seal(seedOf(n))`, где поддельные случайные байты — `1, 2, 3, …`.
fn sealed_of(n: usize) -> String {
    let fill = (n * 2 + 1) as u8;
    attest_seal::seal_with(&seed_of(n), &[fill; 16], &[fill + 1; 32])
}

#[test]
fn seal_records_and_corpus_match_node() {
    let data = fixture("attestation.json");
    let seal = &data["seal"];
    let mut m = Mismatches::default();
    for (n, case) in items(&seal["sealed"]).iter().enumerate() {
        m.check(&json!(hex(&seed_of(n))), &case["seed"], format!("seed {n}"));
        m.check(&json!(sealed_of(n)), &case["sealed"], format!("seal {n}"));
    }
    for case in items(&seal["unseal"]) {
        let text = str_of(&case[0]);
        let actual = attest_seal::unseal(Some(text)).map_or(Value::Null, |seed| json!(hex(&seed)));
        m.check(&actual, &case[1], format!("unseal {text:?}"));
    }
    m.check(
        &json!(attest_seal::unseal(None)),
        &seal["unsealUndefined"],
        "unseal(undefined)",
    );
    let short = attest_seal::seal(&[0u8; 31]).unwrap_err().to_string();
    m.check(&json!(short), &seal["shortSeed"], "seal(31 байт)");
    m.finish("запись ключа");

    // Самотест 0.4.7: seed не виден в записи, соль и «перец» случайны.
    let seed = seed_of(0);
    let sealed = attest_seal::seal(&seed).unwrap();
    assert!(!sealed.contains(&hex(&seed)) && !sealed.contains(&STANDARD.encode(seed)));
    assert_eq!(attest_seal::unseal(Some(&sealed)), Some(seed));
    assert_ne!(
        attest_seal::seal(&seed).unwrap(),
        sealed,
        "соль и «перец» случайны"
    );
}

#[test]
fn signatures_and_build_headers_match_node() {
    let data = fixture("attestation.json");
    let mut m = Mismatches::default();
    for (n, case) in items(&data["sign"]).iter().enumerate() {
        let key = BuildKey::new(Some(&sealed_of(n)));
        assert!(key.available());
        m.check(
            &json!(key.public_key().map(|k| hex(&k))),
            &case["publicKey"],
            format!("ключ {n}"),
        );
        for pair in items(&case["signatures"]) {
            let message = str_of(&pair[0]);
            let signature = key.sign(message.as_bytes()).unwrap();
            m.check(
                &json!(URL_SAFE_NO_PAD.encode(signature)),
                &pair[1],
                format!("подпись {n} {message:.20?}"),
            );
        }
    }
    for case in items(&data["headers"]) {
        let text = str_of(&case[0]);
        m.check(
            &json!(valid_build_header(text)),
            &case[1],
            format!("заголовок {text:?}"),
        );
    }
    m.finish("подписи и заголовки");

    // Самотест 0.4.7: подпись проверяется открытой половиной; без записи ключа нет.
    let key = BuildKey::new(Some(&sealed_of(0)));
    let signature = Signature::from_bytes(&key.sign(b"hello").unwrap());
    let public = VerifyingKey::from_bytes(&public_of(&seed_of(0))).unwrap();
    assert!(public.verify(b"hello", &signature).is_ok());
    assert!(public.verify(b"hellO", &signature).is_err());
    for empty in [Some(""), Some("мусор"), None] {
        let key = BuildKey::new(empty);
        assert!(!key.available());
        assert_eq!(key.sign(b"x"), Err(NoKey));
        assert_eq!(NoKey.to_string(), "ATTEST_NO_KEY");
    }
}

// --- Двойник сервера (как в скрипте) ---

const SECRET_LABEL: &str = "hlsr-test-secret";

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

struct Server {
    bytes: Vec<u8>,
    public_key: [u8; 32],
    header: String,
    clock: FakeClock,
    status: Option<Value>,
    answers: Vec<Value>,
    picked: Vec<usize>,
    chunk: usize,
    nonce: u32,
    log: Vec<Value>,
}

impl Server {
    fn mac(&self, kind: &str, body: &str) -> String {
        let secret: [u8; 32] = Sha256::digest(SECRET_LABEL.as_bytes()).into();
        URL_SAFE_NO_PAD.encode(hmac_sha256(&secret, format!("{kind}.{body}").as_bytes()))
    }

    fn sign_token(&self, kind: &str, data: Value) -> String {
        let mut object = Map::new();
        object.insert("k".to_owned(), json!(kind));
        object.extend(data.as_object().unwrap().clone());
        let body = URL_SAFE_NO_PAD.encode(stringify(&Value::Object(object), ""));
        format!("{body}.{}", self.mac(kind, &body))
    }

    fn read_token(&self, token: &str, kind: &str) -> Option<Value> {
        let mut parts = token.split('.');
        let body = parts.next().unwrap_or("");
        let tag = parts.next().unwrap_or("");
        if tag != self.mac(kind, body) {
            return None;
        }
        json_parse(&String::from_utf8(URL_SAFE_NO_PAD.decode(body).ok()?).ok()?).ok()
    }

    fn rows(&self) -> Vec<(usize, usize)> {
        (0..self.bytes.len())
            .step_by(self.chunk)
            .map(|offset| (offset, self.chunk.min(self.bytes.len() - offset)))
            .collect()
    }

    fn handle(&mut self, url: &str, options: &RequestOptions) -> Result<Response, TransportError> {
        let headers: Map<String, Value> = options
            .headers
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        self.log.push(json!({
            "url": url, "method": options.method, "headers": headers, "body": options.body,
            "timeoutMs": options.timeout_ms, "maxResponseBytes": options.max_response_bytes,
            "launcherVersion": options.launcher_version,
        }));
        let reply = |status: u16, data: Value| Response {
            status,
            headers: Headers::default(),
            body: match data {
                Value::String(text) => text,
                other => stringify(&other, ""),
            },
        };
        if !self.answers.is_empty() {
            let answer = self.answers.remove(0);
            return match answer["throw"].as_str() {
                Some("TELEMETRY_NETWORK") => Err(TransportError::Network),
                Some("TELEMETRY_TIMEOUT") => Err(TransportError::Timeout),
                Some(other) => panic!("неизвестный сбой {other}"),
                None => Ok(reply(
                    answer["status"].as_u64().unwrap() as u16,
                    answer["body"].clone(),
                )),
            };
        }
        if let Some(status) = &self.status {
            return Ok(reply(
                status["code"].as_u64().unwrap() as u16,
                json!({ "error": status["error"] }),
            ));
        }
        let body = json_parse(options.body.as_deref().unwrap_or("{}")).unwrap();
        let build = options
            .headers
            .iter()
            .find(|(name, _)| name == "x-hlsr-build")
            .map(|(_, v)| v.as_str());
        if build != Some(self.header.as_str()) {
            return Ok(reply(403, json!({ "error": "CLIENT_MODIFIED" })));
        }
        let rows = self.rows();
        let now = self.clock.get();
        if url.ends_with("/v1/attest/challenge") {
            self.nonce += 1;
            let token = self.sign_token("c", json!({
                "i": self.picked, "n": format!("{:08x}", self.nonce), "e": now.div_euclid(1000) + 90,
            }));
            let ranges: Vec<Value> = self
                .picked
                .iter()
                .map(|&i| json!([rows[i].0, rows[i].1]))
                .collect();
            return Ok(reply(200, json!({ "challenge": token, "ranges": ranges })));
        }
        let token = body["challenge"].as_str().unwrap_or("");
        let Some(data) = self.read_token(token, "c") else {
            return Ok(reply(403, json!({ "error": "CLIENT_MODIFIED" })));
        };
        let hashes: Vec<String> = items(&data["i"])
            .iter()
            .map(|i| {
                let (offset, length) = rows[i.as_u64().unwrap() as usize];
                sha_hex(&self.bytes[offset..offset + length])
            })
            .collect();
        let message = ["hlsr-attest/1", &self.header, token, &hashes.join(",")].join("\n");
        let signature = URL_SAFE_NO_PAD
            .decode(body["signature"].as_str().unwrap_or(""))
            .ok()
            .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
            .map(|bytes| Signature::from_bytes(&bytes));
        let public = VerifyingKey::from_bytes(&self.public_key).unwrap();
        if !signature.is_some_and(|signature| public.verify(message.as_bytes(), &signature).is_ok())
        {
            return Ok(reply(403, json!({ "error": "CLIENT_MODIFIED" })));
        }
        self.nonce += 1;
        let ticket = self.sign_token(
            "t",
            json!({ "n": format!("{:08x}", self.nonce), "e": now.div_euclid(1000) + 900 }),
        );
        Ok(reply(200, json!({ "ticket": ticket, "ttl": 900 })))
    }
}

struct ServerTransport(Mutex<Server>);

impl Transport for ServerTransport {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        let result = self.0.lock().unwrap().handle(url, options);
        Box::pin(async move { result })
    }
}

/// Ключ сборки, который запоминает подписанные сообщения.
struct RecordingKey {
    key: BuildKey,
    signed: Mutex<Vec<String>>,
}

impl AttestKey for RecordingKey {
    fn available(&self) -> bool {
        self.key.available()
    }
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], NoKey> {
        self.signed
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(message).into_owned());
        self.key.sign(message)
    }
}

/// Файл сборки скрипта: `(i·31 + 7) mod 251`.
fn build_bytes(length: usize) -> Vec<u8> {
    (0..length).map(|i| ((i * 31 + 7) % 251) as u8).collect()
}

fn write_build(file: &Path, bytes: &[u8]) {
    std::fs::write(file, bytes).unwrap();
    let mtime = std::time::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 123_000_000);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

fn outcome(
    result: Result<Option<String>, hlsr_online::security::attestation::ClientModified>,
) -> Value {
    match result {
        Ok(ticket) => json!({ "ticket": ticket }),
        Err(error) => json!({ "error": error.to_string() }),
    }
}

#[tokio::test]
async fn build_attestation_matches_node_scenarios() {
    let data = fixture("attestation.json");
    let chunk = data["chunk"].as_u64().unwrap() as usize;
    let bytes = build_bytes(data["buildLength"].as_u64().unwrap() as usize);
    assert_eq!(sha_hex(&bytes), str_of(&data["buildSha256"]));
    let mut m = Mismatches::default();
    for scenario in items(&data["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("hlsr");
        write_build(&file, &bytes);
        let clock = FakeClock::new(data["now"].as_i64().unwrap());
        let packaged = scenario["packaged"] != json!(false);
        let identity = BuildIdentity::with_file("0.4.5", "linux", packaged, &file);
        let header = identity.header().await;
        m.check(
            &json!(header),
            &scenario["header"],
            format!("{name}: заголовок"),
        );
        if let Some(at) = scenario["tamper"]["at"].as_u64() {
            let mut forged = bytes.clone();
            forged[at as usize] ^= 0xff;
            write_build(&file, &forged);
        }
        let server = &scenario["server"];
        let public = public_of(&seed_of(if server["otherKey"] == json!(true) {
            1
        } else {
            0
        }));
        let transport = Arc::new(ServerTransport(Mutex::new(Server {
            bytes: bytes.clone(),
            public_key: public,
            header: if server["otherHeader"] == json!(true) {
                format!("linux/0.4.5/{}", "0".repeat(64))
            } else {
                header.clone()
            },
            clock: clock.clone(),
            status: server.get("status").cloned(),
            answers: server["answers"].as_array().cloned().unwrap_or_default(),
            picked: server["picked"]
                .as_array()
                .map_or(vec![0, 2, 4, 5], |picked| {
                    picked
                        .iter()
                        .map(|i| i.as_u64().unwrap() as usize)
                        .collect()
                }),
            chunk,
            nonce: 0,
            log: Vec::new(),
        })));
        let sealed = scenario["key"]
            .as_str()
            .map_or_else(|| sealed_of(0), str::to_owned);
        let key = Arc::new(RecordingKey {
            key: BuildKey::new(Some(&sealed)),
            signed: Mutex::new(Vec::new()),
        });
        let logger = Arc::new(MemoryLogger::default());
        let attest = BuildAttestation::new(AttestationOptions {
            identity: Some(identity.clone()),
            key: Some(key.clone()),
            transport: transport.clone(),
            api_base: "https://api.test".to_owned(),
            launcher_version: "0.4.5".to_owned(),
            clock: clock.clock(),
            logger: logger.clone(),
        });
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "{name}"
        );
        for (index, (step, expected)) in items(&scenario["steps"])
            .iter()
            .zip(items(&scenario["results"]))
            .enumerate()
        {
            let force = step["force"] == json!(true);
            let result = match str_of(&step["op"]) {
                "ticket" => outcome(attest.ticket(force).await),
                "parallel" => {
                    let handles: Vec<_> = (0..step["count"].as_u64().unwrap())
                        .map(|_| {
                            let attest = attest.clone();
                            tokio::spawn(async move { attest.ticket(force).await })
                        })
                        .collect();
                    let mut results = Vec::new();
                    for handle in handles {
                        results.push(outcome(handle.await.unwrap()));
                    }
                    Value::Array(results)
                }
                "advance" => {
                    clock.advance(step["ms"].as_i64().unwrap());
                    Value::Null
                }
                "append" => {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(&file)
                        .unwrap()
                        .write_all("новая версия".as_bytes())
                        .unwrap();
                    Value::Null
                }
                "header" => json!(identity.header().await),
                "enabled" => json!(attest.enabled()),
                "readRange" => match identity
                    .read_range(&step["range"][0], &step["range"][1])
                    .await
                {
                    Ok(bytes) => json!({ "sha256": sha_hex(&bytes), "length": bytes.len() }),
                    Err(error) => json!({ "error": error.message, "code": error.code }),
                },
                other => panic!("неизвестный шаг {other}"),
            };
            let signed = std::mem::take(&mut *key.signed.lock().unwrap());
            let requests = std::mem::take(&mut transport.0.lock().unwrap().log);
            let actual = json!({ "result": result, "logs": log_lines(&logger), "signed": signed, "requests": requests });
            m.check(&actual, expected, format!("{name}: шаг {index} {step}"));
        }
    }
    m.finish("сценарии аттестации");
}

/// Linux: отпечаток запущенного файла — `/proc/self/exe`, участки читаются из него же.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn running_executable_is_hashed_once() {
    let identity = BuildIdentity::new("0.4.5", true);
    assert_eq!(identity.file(), Path::new("/proc/self/exe"));
    let header = identity.header().await;
    let own = std::fs::read("/proc/self/exe").unwrap();
    assert_eq!(header, format!("linux/0.4.5/{}", sha_hex(&own)));
    assert!(valid_build_header(&header));
    let head = identity.read_range(&json!(0), &json!(4)).await.unwrap();
    assert_eq!(head, b"\x7fELF");
    let dev = BuildIdentity::new("0.4.5", false);
    assert_eq!(dev.header().await, "linux/0.4.5/dev");
}
