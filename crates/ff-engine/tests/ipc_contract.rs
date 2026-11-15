use std::io::{self, Cursor, Read};

use ff_engine::api::{EngineError, SearchOptions, SearchRequest, SortSpec};
use ff_engine::ipc::{
    read_request, read_response, write_request, write_response, Command, ErrorCode, Message, Reply,
    RpcError, MAX_FRAME_BYTES, MAX_QUERY_BYTES, MAX_RANGE_ROWS,
};

fn search() -> Command {
    Command::Search(SearchRequest {
        query: "中国 report".into(),
        sort: SortSpec::default(),
        max_hits: Some(10_000),
        opts: SearchOptions::default(),
    })
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

#[test]
fn search_roundtrip_preserves_contract_and_little_endian_length() {
    let request = Message::new(42, search());
    let mut bytes = Vec::new();
    write_request(&mut bytes, &request).unwrap();
    assert_eq!(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize,
        bytes.len() - 4
    );
    let decoded = read_request(&mut Cursor::new(bytes)).unwrap().unwrap();
    assert_eq!(decoded.request_id, 42);
    let Command::Search(req) = decoded.body else {
        panic!("wrong method")
    };
    assert_eq!(req.query, "中国 report");
    assert!(req.opts.pinyin);
    assert_eq!(req.max_hits, Some(10_000));
}

#[test]
fn all_commands_roundtrip_without_executing_actions() {
    use ff_engine::OpenAction;
    for command in [
        Command::Range {
            id: 5,
            offset: 10,
            limit: 40,
        },
        Command::Cancel { id: 5 },
        Command::Status,
        Command::Rescan { volume: None },
        Command::Rescan {
            volume: Some("C:".into()),
        },
        Command::Open {
            id: 5,
            index: 0,
            action: OpenAction::OpenFolder,
        },
    ] {
        let request = Message::new(1, command);
        let mut bytes = Vec::new();
        write_request(&mut bytes, &request).unwrap();
        let received = read_request(&mut Cursor::new(bytes)).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&received).unwrap(),
            serde_json::to_value(&request).unwrap()
        );
    }
}

#[test]
fn clean_eof_is_different_from_partial_header_or_body() {
    assert!(read_request(&mut Cursor::new([])).unwrap().is_none());
    for bytes in [vec![1], vec![1, 0, 0], vec![5, 0, 0, 0, b'{']] {
        assert_eq!(
            read_request(&mut Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn invalid_lengths_are_rejected_before_reading_payload() {
    for len in [0_u32, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        assert_eq!(
            read_request(&mut Cursor::new(len.to_le_bytes()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn malformed_json_unknown_method_and_version_are_rejected() {
    for payload in [
        b"{".as_slice(),
        b"\xff",
        br#"{"protocol_version":1,"request_id":1,"body":{"method":"shell"}}"#,
        br#"{"protocol_version":2,"request_id":1,"body":{"method":"status"}}"#,
        br#"{"protocol_version":1,"request_id":0,"body":{"method":"status"}}"#,
    ] {
        assert_eq!(
            read_request(&mut Cursor::new(frame(payload)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn query_and_range_limits_apply_to_received_and_sent_requests() {
    let Command::Search(mut req) = search() else {
        unreachable!()
    };
    req.query = "x".repeat(MAX_QUERY_BYTES + 1);
    for command in [
        Command::Search(req),
        Command::Range {
            id: 1,
            offset: 0,
            limit: MAX_RANGE_ROWS + 1,
        },
        Command::Range {
            id: 1,
            offset: usize::MAX,
            limit: 1,
        },
        Command::Rescan {
            volume: Some(String::new()),
        },
    ] {
        let request = Message::new(1, command);
        let mut bytes = Vec::new();
        assert!(write_request(&mut bytes, &request).is_err());
        assert!(bytes.is_empty());
        let json = serde_json::to_vec(&request).unwrap();
        assert!(read_request(&mut Cursor::new(frame(&json))).is_err());
    }
}

#[test]
fn concatenated_frames_are_read_one_at_a_time() {
    let mut bytes = Vec::new();
    write_request(&mut bytes, &Message::new(1, Command::Status)).unwrap();
    write_request(&mut bytes, &Message::new(2, Command::Cancel { id: 9 })).unwrap();
    let mut stream = Cursor::new(bytes);
    assert_eq!(read_request(&mut stream).unwrap().unwrap().request_id, 1);
    assert_eq!(read_request(&mut stream).unwrap().unwrap().request_id, 2);
    assert!(read_request(&mut stream).unwrap().is_none());
}

struct Fragmented<R>(R);
impl<R: Read> Read for Fragmented<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let len = output.len().min(1);
        self.0.read(&mut output[..len])
    }
}

#[test]
fn fragmented_transport_reads_are_supported() {
    let mut bytes = Vec::new();
    write_request(&mut bytes, &Message::new(7, search())).unwrap();
    let mut reader = Fragmented(Cursor::new(bytes));
    assert_eq!(read_request(&mut reader).unwrap().unwrap().request_id, 7);
}

#[test]
fn oversized_responses_write_no_partial_header() {
    let response = Message::new(
        1,
        Reply::Error(RpcError {
            code: ErrorCode::Internal,
            message: "x".repeat(MAX_FRAME_BYTES),
        }),
    );
    let mut bytes = Vec::new();
    assert!(write_response(&mut bytes, &response).is_err());
    assert!(bytes.is_empty());
}

#[test]
fn structured_error_response_preserves_transport_correlation() {
    let response = Message::new(23, Reply::Error(EngineError::NotFound.into()));
    let mut bytes = Vec::new();
    write_response(&mut bytes, &response).unwrap();
    let response = read_response(&mut Cursor::new(bytes)).unwrap().unwrap();
    assert_eq!(response.request_id, 23);
    let Reply::Error(error) = response.body else {
        panic!("wrong reply")
    };
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn pipe_preserves_filetime_and_utf16_highlight_ranges_exactly() {
    let response = Message::new(
        8,
        Reply::Range(vec![ff_engine::ResultRow {
            name: "😀report".into(),
            dir: "C:\\".into(),
            is_dir: false,
            size: Some(10),
            mtime: Some(134_116_992_000_000_001),
            name_matches: std::iter::once(2..8).collect(),
        }]),
    );
    let mut bytes = Vec::new();
    write_response(&mut bytes, &response).unwrap();
    let decoded = read_response(&mut Cursor::new(bytes)).unwrap().unwrap();
    let Reply::Range(rows) = decoded.body else {
        panic!("wrong reply")
    };
    assert_eq!(rows[0].mtime, Some(134_116_992_000_000_001));
    assert_eq!(rows[0].name_matches[0], 2..8);
}
