//! `splash` 模块单元测试：仅断言编译期嵌入的动画帧文件是否就绪。

use super::{splash_frame_count, splash_frames_loaded, FRAME_COUNT};

#[test]
fn splash_animation_frames_are_embedded() {
    assert_eq!(splash_frame_count(), FRAME_COUNT);
    assert!(splash_frames_loaded());
}
