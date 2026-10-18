use quotarail::{
    error::ErrorKind,
    protocol::{MAX_MESSAGE_BYTES, read_frame},
};
use tokio::io::BufReader;

#[tokio::test(flavor = "current_thread")]
async fn multiple_frames_remain_distinct_and_accept_crlf() {
    let bytes = b"{\"id\":1,\"result\":{}}\r\n{\"id\":2,\"result\":{}}\n";
    let mut reader = BufReader::with_capacity(3, &bytes[..]);
    assert_eq!(read_frame(&mut reader).await.unwrap()["id"], 1);
    assert_eq!(read_frame(&mut reader).await.unwrap()["id"], 2);
}

#[tokio::test(flavor = "current_thread")]
async fn unterminated_and_oversized_frames_fail_without_returning_raw_content() {
    let mut reader = BufReader::new(&b"{\"secret\":\"SYNTHETIC\"}"[..]);
    let error = read_frame(&mut reader).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::ChildExited);
    assert!(!error.to_string().contains("SYNTHETIC"));
    let oversized = vec![b'x'; MAX_MESSAGE_BYTES + 1];
    let mut reader = BufReader::with_capacity(1024, &oversized[..]);
    assert_eq!(
        read_frame(&mut reader).await.unwrap_err().kind,
        ErrorKind::MessageTooLarge
    );
}
