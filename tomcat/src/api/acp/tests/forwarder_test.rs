use super::*;

#[tokio::test]
async fn close_record_follows_an_unterminated_host_frame_and_ack_is_private() {
    let (client, server) = tokio::io::duplex(1024);
    let (output, mut captured) = tokio::io::duplex(1024);
    let peer = async move {
        let mut server = BufReader::new(server);
        let mut line = String::new();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, "{\"jsonrpc\":\"2.0\",\"id\":1}\n");
        line.clear();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, format!("{CLOSE}\n"));
        server
            .write_all(
                format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\n{CLOSED}\n").as_bytes(),
            )
            .await
            .unwrap();
    };
    let (result, ()) = tokio::join!(
        forward(client, &b"{\"jsonrpc\":\"2.0\",\"id\":1}"[..], output),
        peer
    );
    result.unwrap();
    let mut bytes = String::new();
    captured.read_to_string(&mut bytes).await.unwrap();
    assert_eq!(bytes, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
}

#[tokio::test]
async fn failed_cleanup_ack_is_an_error_not_a_successful_exit() {
    let (client, mut server) = tokio::io::duplex(1024);
    server
        .write_all(format!("{CLOSE_FAILED}\n").as_bytes())
        .await
        .unwrap();
    let error = forward(client, tokio::io::empty(), tokio::io::sink())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cleanup failed"));
}

#[tokio::test]
async fn missing_cleanup_ack_has_a_bounded_honest_timeout() {
    let (client, server) = tokio::io::duplex(1024);
    let started = std::time::Instant::now();
    // Keep the peer open but deliberately withhold the acknowledgement.
    let result = tokio::time::timeout(
        Duration::from_secs(13),
        forward(client, tokio::io::empty(), tokio::io::sink()),
    )
    .await
    .unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains("timed out after 10s"), "{error}");
    assert!(error.contains("not confirmed"));
    assert!(started.elapsed() >= Duration::from_secs(10));
    drop(server);
}
