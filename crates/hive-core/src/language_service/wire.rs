use super::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Read};

pub(super) const FRAME_LIMIT: usize = 8 * 1024 * 1024;
const HEADER_LIMIT: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Id {
    Number(i64),
    String(String),
}

pub(super) enum Incoming {
    Response(Id, Result<Value, Error>),
    Call {
        id: Option<Id>,
        method: String,
        params: Value,
    },
    /// A whole message the client cannot use, said why. The stream is still in step, so one
    /// such message must not end the session.
    Ignored(&'static str),
}

pub(super) fn read(reader: &mut impl BufRead) -> Result<Option<Incoming>, Error> {
    let mut length = None;
    let mut consumed = 0;
    loop {
        let mut line = Vec::new();
        let count = (&mut *reader)
            .take((HEADER_LIMIT - consumed) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|e| Error::Protocol(e.to_string()))?;
        if count == 0 && consumed == 0 {
            return Ok(None);
        }
        consumed += count;
        if count == 0 || consumed >= HEADER_LIMIT || !line.ends_with(b"\r\n") {
            return Err(Error::Protocol("Truncated or oversized header".into()));
        }
        if line == b"\r\n" {
            break;
        }
        let line =
            std::str::from_utf8(&line).map_err(|_| Error::Protocol("Non-ASCII header".into()))?;
        let Some((name, value)) = line.split_once(':') else {
            return Err(Error::Protocol("Malformed header".into()));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(Error::Protocol("Duplicate Content-Length".into()));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| Error::Protocol("Invalid Content-Length".into()))?,
            );
        }
    }
    let length = length.ok_or_else(|| Error::Protocol("Missing Content-Length".into()))?;
    if length > FRAME_LIMIT {
        // A huge answer, such as every symbol of a large project: its request times out
        // instead of the server being stopped.
        let skipped = std::io::copy(
            &mut (&mut *reader).take(length as u64),
            &mut std::io::sink(),
        )
        .map_err(|e| Error::Protocol(e.to_string()))?;
        if skipped < length as u64 {
            return Err(Error::Protocol("Truncated oversized frame".into()));
        }
        return Ok(Some(Incoming::Ignored("frame exceeds 8 MiB")));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|e| Error::Protocol(e.to_string()))?;
    parse(&body).map(Some)
}

fn parse(bytes: &[u8]) -> Result<Incoming, Error> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| Error::Protocol(e.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::Protocol("Expected JSON-RPC object".into()))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(Error::Protocol("Expected JSON-RPC 2.0".into()));
    }
    // JSON-RPC answers a request it could not read with `"id": null`.
    let id = object
        .get("id")
        .filter(|id| !id.is_null())
        .map(|id| serde_json::from_value(id.clone()).map_err(|e| Error::Protocol(e.to_string())))
        .transpose()?;
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        if object.contains_key("result") || object.contains_key("error") {
            return Err(Error::Protocol("Call contains a response".into()));
        }
        return Ok(Incoming::Call {
            id,
            method: method.to_owned(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        });
    }
    let Some(id) = id else {
        return Ok(Incoming::Ignored("response without an id"));
    };
    match (object.get("result"), object.get("error")) {
        (Some(result), None) => Ok(Incoming::Response(id, Ok(result.clone()))),
        (None, Some(error)) => {
            #[derive(Deserialize)]
            struct Failure {
                code: i64,
                message: String,
            }
            let error: Failure = serde_json::from_value(error.clone())
                .map_err(|e| Error::Protocol(e.to_string()))?;
            Ok(Incoming::Response(
                id,
                Err(Error::Request {
                    code: error.code,
                    message: error.message,
                }),
            ))
        }
        _ => Err(Error::Protocol(
            "Response must contain result or error".into(),
        )),
    }
}

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(value).map_err(|e| Error::Protocol(e.to_string()))?;
    if body.len() > FRAME_LIMIT {
        return Err(Error::Protocol("Outgoing frame exceeds 8 MiB".into()));
    }
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_count_bytes_preserve_unicode_and_allow_multiple_messages() {
        let mut bytes =
            encode(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":"😀日本語"})).unwrap();
        bytes.extend(encode(&serde_json::json!({"jsonrpc":"2.0","id":"server","method":"workspace/configuration","params":{}})).unwrap());
        let mut reader = std::io::Cursor::new(bytes);
        assert!(
            matches!(read(&mut reader).unwrap(),Some(Incoming::Response(Id::Number(1),Ok(value))) if value == "😀日本語")
        );
        assert!(matches!(
            read(&mut reader).unwrap(),
            Some(Incoming::Call {
                id: Some(Id::String(_)),
                ..
            })
        ));
        assert!(read(&mut reader).unwrap().is_none());
    }
    #[test]
    fn an_answer_nobody_can_match_or_hold_is_skipped_and_the_stream_goes_on() {
        let mut bytes = encode(&serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})).unwrap();
        let huge = "x".repeat(FRAME_LIMIT);
        bytes.extend(encode(&serde_json::json!({"jsonrpc":"2.0","id":2,"result":"ok"})).unwrap());
        bytes.extend(format!("Content-Length: {}\r\n\r\n\"{huge}\"", huge.len() + 2).into_bytes());
        bytes.extend(encode(&serde_json::json!({"jsonrpc":"2.0","id":null,"method":"window/logMessage","params":{}})).unwrap());
        let mut reader = std::io::Cursor::new(bytes);
        assert!(matches!(
            read(&mut reader).unwrap(),
            Some(Incoming::Ignored(_))
        ));
        assert!(matches!(
            read(&mut reader).unwrap(),
            Some(Incoming::Response(Id::Number(2), Ok(_)))
        ));
        assert!(matches!(
            read(&mut reader).unwrap(),
            Some(Incoming::Ignored(_))
        ));
        assert!(matches!(
            read(&mut reader).unwrap(),
            Some(Incoming::Call { id: None, .. })
        ));
        assert!(read(&mut reader).unwrap().is_none());
    }

    #[test]
    fn malformed_truncated_duplicate_and_oversized_frames_fail_boundedly() {
        for input in [
            "Content-Length: 999999999\r\n\r\n",
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "Content-Length: 100\r\n\r\n{}",
            "garbage\r\n\r\n",
            "Content-Length: -1\r\n\r\n",
        ] {
            assert!(read(&mut std::io::Cursor::new(input)).is_err());
        }
        for value in [
            serde_json::json!({"id":1,"result":null}),
            serde_json::json!({"jsonrpc":"2.0","id":1}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":null,"error":{"code":1,"message":"bad"}}),
        ] {
            assert!(read(&mut std::io::Cursor::new(encode(&value).unwrap())).is_err());
        }
    }
}
