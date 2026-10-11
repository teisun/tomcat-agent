use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use super::OAuthCallbackListener;

#[tokio::test]
async fn binds_a_loopback_callback_with_dynamic_port() {
    let listener = OAuthCallbackListener::bind().await.expect("listener");
    let redirect = listener.redirect_uri().expect("redirect");
    assert!(redirect.starts_with("http://127.0.0.1:"));
    assert!(redirect.ends_with("/callback"));
}

#[tokio::test]
async fn dropping_a_scoped_callback_releases_its_listener() {
    let listener = OAuthCallbackListener::bind().await.unwrap();
    let address = listener.listener.local_addr().unwrap();
    let mut waiting = Box::pin(listener.wait("state"));
    tokio::select! {
        biased;
        _ = &mut waiting => panic!("callback should still be waiting"),
        _ = tokio::task::yield_now() => {},
    }
    drop(waiting);
    let _rebound = tokio::net::TcpListener::bind(address)
        .await
        .expect("listener must not remain in a detached task");
}

#[tokio::test]
async fn callback_pages_keep_status_and_utf8_byte_length_without_echoing_code() {
    use crate::infra::i18n::{tr_in, Locale};
    use tokio::io::AsyncReadExt;
    for (query, status, key) in [
        (
            "code=private-code&state=expected",
            "200 OK",
            "oauth.successPage",
        ),
        ("state=expected", "400 Bad Request", "oauth.codePage"),
        (
            "code=private-code&state=other",
            "400 Bad Request",
            "oauth.statePage",
        ),
    ] {
        let listener = OAuthCallbackListener::bind().await.unwrap();
        let address = listener.listener.local_addr().unwrap();
        let task = tokio::spawn(async move { listener.wait("expected").await });
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let result = task.await.unwrap();
        assert_eq!(result.is_ok(), status == "200 OK");
        let (headers, content) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with(&format!("HTTP/1.1 {status}")));
        assert!(headers.contains(&format!("Content-Length: {}", content.len())));
        assert!(content.contains(&tr_in(Locale::En, key, &[])), "{content}");
        assert!(!content.contains("private-code"));
    }
}

#[tokio::test]
async fn rejects_invalid_state() {
    let listener = OAuthCallbackListener::bind().await.expect("listener");
    let address = listener.listener.local_addr().expect("address");
    let task = tokio::spawn(async move { listener.wait("expected-state").await });
    let mut stream = TcpStream::connect(address)
        .await
        .expect("callback connection");
    stream
        .write_all(b"GET /callback?code=code&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("callback request");
    assert!(task.await.expect("callback task").is_err());
}
