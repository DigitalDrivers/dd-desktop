//! The obs-websocket 5 protocol (rpcVersion 1) as text in and text out; the shell owns the connection. The server
//! says Hello (op 0) with a salt and a challenge when it wants a password; the client answers Identify (op 1) with
//! `base64(sha256(base64(sha256(password + salt)) + challenge))` and gets Identified (op 2), or the connection
//! closes with 4009. Requests (op 6) carry an id the response (op 7) repeats; events (op 5) come in between.
//! Source: obs-websocket `docs/generated/protocol.md` (5.7.6) and `lib/obsws.ps1` of the 2026-11-09 tests.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const OP_HELLO: u64 = 0;
pub const OP_IDENTIFY: u64 = 1;
pub const OP_IDENTIFIED: u64 = 2;
pub const OP_EVENT: u64 = 5;
pub const OP_REQUEST: u64 = 6;
pub const OP_RESPONSE: u64 = 7;
/// The event categories the app subscribes to: outputs (`RecordStateChanged`).
pub const EVENTS_OUTPUTS: u64 = 1 << 6;
/// The close code of a wrong password.
pub const CLOSE_AUTH_FAILED: u16 = 4009;
/// The record output's state once its file is complete.
pub const OUTPUT_STOPPED: &str = "OBS_WEBSOCKET_OUTPUT_STOPPED";

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// The server's greeting, its `d`.
    Hello(Value),
    Identified,
    Event { kind: String, data: Value },
    Response { id: String, result: Result<Value, Failed> },
    /// An op the app does not use.
    Other(u64),
}

/// A request OBS refused: its status code and comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub code: u64,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Not a message of the protocol.
    Malformed,
    /// The server wants a password and the app has none.
    PasswordNeeded,
}

fn sha256_base64(text: &str) -> String {
    STANDARD.encode(Sha256::digest(text.as_bytes()))
}

/// The authentication string for a Hello's salt and challenge.
pub fn auth(password: &str, salt: &str, challenge: &str) -> String {
    sha256_base64(&(sha256_base64(&format!("{password}{salt}")) + challenge))
}

/// The Identify answer to `hello` (a Hello's `d`).
pub fn identify(hello: &Value, password: Option<&str>, events: u64) -> Result<String, ProtocolError> {
    let mut d = json!({ "rpcVersion": 1, "eventSubscriptions": events });
    if let Some(login) = hello.get("authentication") {
        let (salt, challenge) = (login["salt"].as_str().ok_or(ProtocolError::Malformed)?, login["challenge"].as_str().ok_or(ProtocolError::Malformed)?);
        d["authentication"] = auth(password.ok_or(ProtocolError::PasswordNeeded)?, salt, challenge).into();
    }
    Ok(json!({ "op": OP_IDENTIFY, "d": d }).to_string())
}

pub fn request(kind: &str, id: &str, data: Option<&Value>) -> String {
    let mut d = json!({ "requestType": kind, "requestId": id });
    if let Some(data) = data {
        d["requestData"] = data.clone();
    }
    json!({ "op": OP_REQUEST, "d": d }).to_string()
}

pub fn parse(text: &str) -> Result<Message, ProtocolError> {
    let message: Value = serde_json::from_str(text).map_err(|_| ProtocolError::Malformed)?;
    let op = message["op"].as_u64().ok_or(ProtocolError::Malformed)?;
    let d = message.get("d").cloned().unwrap_or(Value::Null);
    Ok(match op {
        OP_HELLO => Message::Hello(d),
        OP_IDENTIFIED => Message::Identified,
        OP_EVENT => Message::Event { kind: d["eventType"].as_str().ok_or(ProtocolError::Malformed)?.to_string(), data: d.get("eventData").cloned().unwrap_or(Value::Null) },
        OP_RESPONSE => {
            let id = d["requestId"].as_str().ok_or(ProtocolError::Malformed)?.to_string();
            let status = &d["requestStatus"];
            let result = if status["result"].as_bool() == Some(true) {
                Ok(d.get("responseData").cloned().unwrap_or_else(|| json!({})))
            } else {
                Err(Failed { code: status["code"].as_u64().unwrap_or(0), comment: status["comment"].as_str().unwrap_or_default().to_string() })
            };
            Message::Response { id, result }
        }
        other => Message::Other(other),
    })
}

/// The record output's new state from a `RecordStateChanged` event's data: the state and, once stopped, the file.
pub fn record_state(data: &Value) -> Option<(String, Option<String>)> {
    Some((data["outputState"].as_str()?.to_string(), data["outputPath"].as_str().map(str::to_string)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Hello of protocol.md's authentication guide.
    fn hello() -> Value {
        json!({ "obsStudioVersion": "30.2.2", "obsWebSocketVersion": "5.5.2", "rpcVersion": 1, "authentication": {
            "challenge": "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=", "salt": "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=" } })
    }

    #[test]
    fn logs_in_as_the_protocol_says() {
        // python3: b64(sha256(b64(sha256("supersecretpassword" + salt)) + challenge)); the guide's own Identify example
        // is not computed from its Hello
        assert_eq!(auth("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=", "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="), "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4=");
        let sent: Value = serde_json::from_str(&identify(&hello(), Some("supersecretpassword"), EVENTS_OUTPUTS).unwrap()).unwrap();
        assert_eq!(sent, json!({ "op": 1, "d": { "rpcVersion": 1, "eventSubscriptions": 64, "authentication": "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4=" } }));
        assert_eq!(identify(&hello(), None, 64), Err(ProtocolError::PasswordNeeded));
        let open: Value = serde_json::from_str(&identify(&json!({ "rpcVersion": 1 }), None, 64).unwrap()).unwrap();
        assert_eq!(open["d"].get("authentication"), None);
    }

    #[test]
    fn reads_what_obs_sends() {
        assert_eq!(parse(&json!({ "op": 0, "d": hello() }).to_string()), Ok(Message::Hello(hello())));
        assert_eq!(parse(r#"{"op":2,"d":{"negotiatedRpcVersion":1}}"#), Ok(Message::Identified));
        // GetVersion as OBS 33.0.1 answered on Joe's PC (shortened)
        let version = r#"{"op":7,"d":{"requestType":"GetVersion","requestId":"dd-1","requestStatus":{"result":true,"code":100},"responseData":{"obsVersion":"33.0.1","obsWebSocketVersion":"5.7.6","rpcVersion":1,"platformDescription":"Windows 11 Version 23H2"}}}"#;
        match parse(version).unwrap() {
            Message::Response { id, result: Ok(data) } => assert_eq!((id.as_str(), data["obsVersion"].as_str()), ("dd-1", Some("33.0.1"))),
            other => panic!("{other:?}"),
        }
        let refused = r#"{"op":7,"d":{"requestType":"GetSceneItemId","requestId":"dd-9","requestStatus":{"result":false,"code":600,"comment":"No scene items were found in the specified scene by that criteria."}}}"#;
        assert_eq!(parse(refused).unwrap(), Message::Response { id: "dd-9".into(), result: Err(Failed { code: 600, comment: "No scene items were found in the specified scene by that criteria.".into() }) });
        let empty = r#"{"op":7,"d":{"requestType":"StartRecord","requestId":"dd-3","requestStatus":{"result":true,"code":100}}}"#;
        assert_eq!(parse(empty).unwrap(), Message::Response { id: "dd-3".into(), result: Ok(json!({})) });
        let stopped = r#"{"op":5,"d":{"eventType":"RecordStateChanged","eventIntent":64,"eventData":{"outputActive":false,"outputState":"OBS_WEBSOCKET_OUTPUT_STOPPED","outputPath":"C:/projekte/digitaldrivers/testabend/video/DD_Test 2026-11-09 20-58-01.mp4"}}}"#;
        let Message::Event { kind, data } = parse(stopped).unwrap() else { panic!() };
        assert_eq!(kind, "RecordStateChanged");
        assert_eq!(record_state(&data), Some((OUTPUT_STOPPED.to_string(), Some("C:/projekte/digitaldrivers/testabend/video/DD_Test 2026-11-09 20-58-01.mp4".to_string()))));
        assert_eq!(parse(r#"{"op":9,"d":{}}"#), Ok(Message::Other(9)));
        assert_eq!(parse("{}"), Err(ProtocolError::Malformed));
    }

    #[test]
    fn asks_with_an_id_and_data() {
        let sent: Value = serde_json::from_str(&request("GetOutputStatus", "dd-7", Some(&json!({ "outputName": "adv_file_output" })))).unwrap();
        assert_eq!(sent, json!({ "op": 6, "d": { "requestType": "GetOutputStatus", "requestId": "dd-7", "requestData": { "outputName": "adv_file_output" } } }));
        let bare: Value = serde_json::from_str(&request("GetStats", "dd-8", None)).unwrap();
        assert_eq!(bare["d"].get("requestData"), None);
    }
}
