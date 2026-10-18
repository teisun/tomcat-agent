use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{read_line_from, Input, ReadLineError};

#[cfg(unix)]
fn input(reader: &std::io::PipeReader) -> Input<'_> {
    std::os::fd::AsFd::as_fd(reader)
}

#[cfg(windows)]
fn input(reader: &std::io::PipeReader) -> Input<'_> {
    std::os::windows::io::AsHandle::as_handle(reader)
}

fn never_stop() -> Option<()> {
    None
}

#[test]
fn waits_without_input_until_stopped_then_leaves_next_line_for_next_reader() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let reader = Arc::new(reader);
    let stop = Arc::new(AtomicBool::new(false));
    let first = std::thread::spawn({
        let (reader, stop) = (reader.clone(), stop.clone());
        move || {
            read_line_from(input(&reader), || {
                stop.load(Ordering::SeqCst).then_some("stop")
            })
        }
    });
    std::thread::sleep(Duration::from_millis(150));
    assert!(!first.is_finished(), "没有输入时必须一直等，不能有超时");

    let stopped_at = Instant::now();
    stop.store(true, Ordering::SeqCst);
    assert_eq!(first.join().unwrap(), Err(ReadLineError::Stopped("stop")));
    assert!(stopped_at.elapsed() < Duration::from_millis(200));

    writer.write_all(b"2\n").unwrap();
    assert_eq!(
        read_line_from(input(&reader), never_stop),
        Ok("2\n".to_string())
    );
}

#[test]
fn reads_one_whole_chinese_line_at_a_time() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer
        .write_all("选 Docker 部署\n下一行\n".as_bytes())
        .unwrap();
    assert_eq!(
        read_line_from(input(&reader), never_stop),
        Ok("选 Docker 部署\n".to_string())
    );
    assert_eq!(
        read_line_from(input(&reader), never_stop),
        Ok("下一行\n".to_string())
    );
}

#[test]
fn closed_writer_reports_disconnected_even_after_a_partial_line() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(writer);
    assert_eq!(
        read_line_from(input(&reader), never_stop),
        Err(ReadLineError::Disconnected)
    );

    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"half").unwrap();
    drop(writer);
    assert_eq!(
        read_line_from(input(&reader), never_stop),
        Err(ReadLineError::Disconnected)
    );
}
