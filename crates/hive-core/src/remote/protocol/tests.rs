use super::*;
use std::os::unix::ffi::OsStringExt;

#[test]
fn paths_roundtrip_without_loss_and_shell_banners_are_bounded() {
    let path = PathBuf::from(std::ffi::OsString::from_vec(
        b"/repo/\xff notes'\n".to_vec(),
    ));
    let encoded = encode(&Request::Browse { path: path.clone() }).unwrap();
    let decoded: Request = read(&mut encoded.as_slice()).unwrap();
    assert!(matches!(decoded, Request::Browse { path: actual } if actual == path));
    let mut banner = b"Welcome to the server\n".to_vec();
    banner.extend(&encoded);
    assert!(decode_response::<Request>(&banner).is_ok());
    let mut too_long = vec![b'x'; PREAMBLE_LIMIT + 1];
    too_long.extend(&encoded);
    assert!(decode_response::<Request>(&too_long).is_err());
}

#[test]
fn truncated_oversized_unknown_version_and_trailing_messages_fail() {
    let encoded = encode(&Request::Index {
        root: "/repo".into(),
    })
    .unwrap();
    for length in 0..encoded.len() {
        assert!(read::<Request>(&mut &encoded[..length]).is_err());
    }
    let mut oversized = MAGIC.to_vec();
    oversized.extend_from_slice(&((MESSAGE_LIMIT + 1) as u32).to_be_bytes());
    assert!(read::<Request>(&mut oversized.as_slice()).is_err());
    let json = br#"{"version":999,"value":{"operation":"index","root":"L3JlcG8="}}"#;
    let mut future = MAGIC.to_vec();
    future.extend_from_slice(&(json.len() as u32).to_be_bytes());
    future.extend_from_slice(json);
    assert!(read::<Request>(&mut future.as_slice()).is_err());
    let mut trailing = encoded.clone();
    trailing.extend(encoded);
    assert!(decode_response::<Request>(&trailing).is_err());
}

#[test]
fn remote_not_found_and_conflict_remain_typed() {
    let host = Host::Ssh {
        target: "test".into(),
    };
    let failure = Failure::from(FileError::Io {
        operation: "read",
        path: "/repo/a".into(),
        source: io::Error::from(io::ErrorKind::NotFound),
    });
    assert!(
        matches!(failure.into_file_error(&host), FileError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound)
    );
    assert!(matches!(
        Failure::from(FileError::Conflict("/repo/a".into())).into_file_error(&host),
        FileError::Conflict(_)
    ));
}

#[test]
fn wrong_operation_and_unsafe_paths_are_not_accepted() {
    let request = Request::Index {
        root: "/project".into(),
    };
    for path in ["../secret", "/etc/passwd", ".git/config", ""] {
        let response = Response::Index(crate::files::FileIndex {
            paths: vec![path.into()],
            truncated: false,
        });
        assert!(!accepts(&request, &response));
    }
    assert!(!accepts(&request, &Response::Done));
    assert!(serde_json::from_value::<Snapshot>(serde_json::json!({"raw": "\u{0000}"})).is_err());
    assert!(
        serde_json::from_value::<Snapshot>(
            serde_json::json!({"raw": "x".repeat(crate::files::MAX_FILE_BYTES+1)})
        )
        .is_err()
    );
}

#[test]
fn snapshot_wire_uses_base64_and_preserves_bom_crlf() {
    let bytes = "\u{feff}hello 世界\r\n".as_bytes();
    let response = Response::Snapshot(Snapshot::parse(std::path::Path::new("a"), bytes).unwrap());
    let json = serde_json::to_value(&response).unwrap();
    assert!(json["value"].is_string());
    let decoded: Response = decode_response(&encode(&response).unwrap()).unwrap();
    let Response::Snapshot(snapshot) = decoded else {
        panic!("expected snapshot")
    };
    assert_eq!(snapshot.bytes(), bytes);
    assert!(
        serde_json::from_value::<Response>(serde_json::json!({"kind":"snapshot","value":"!!!"}))
            .is_err()
    );
}
