# LLM 多模态附件测试 fixtures

供 `tests/openai_responses_integration_tests.rs` 中的 `responses_inline_image_describe_roundtrip`
/ `responses_inline_pdf_input_file_summarize_roundtrip` 真 API roundtrip 测试使用。

测试本身只 `include_str!` 读 `.txt`，不依赖 `python3` / `reportlab` / `base64` 工具
在 CI 环境存在；本目录的脚本仅在**需要重新生成 fixture** 时手动运行。

## 文件清单

| 文件 | 用途 | 大小 |
|------|------|------|
| `sample_image.png` | 一只小狗的 PNG 图片，供 vision 模型描述 | ≈ 46 KB |
| `sample_image_b64.txt` | `sample_image.png` 的 base64 字面量（不带 data URL 前缀） | ≈ 62 KB |
| `sample_pdf_b64.txt` | reportlab 生成的单页 PDF 的 base64 字面量 | ≈ 1.8 KB |
| `gen_sample_pdf.py` | 一次性 PDF 生成脚本，约 15 行 | — |

## 图片来源 / 授权

`sample_image.png` 来源：[Unsplash photo by Joe Caione](https://unsplash.com/photos/qO-PIF84Vxg)
（原图 `https://images.unsplash.com/photo-1543466835-00a7907e9de1`）。
Unsplash License（使用、修改、商用均允许，无需署名）：<https://unsplash.com/license>。

下载并在本仓内做了一次 `sips -Z 192 -s format png` 的转换缩放，原始 JPEG 未入仓。

## 复跑命令

```bash
# 重新拉取并转换图片（macOS）
curl -sSL "https://images.unsplash.com/photo-1543466835-00a7907e9de1?w=256&q=80&fm=jpg" -o /tmp/puppy.jpg
sips -Z 192 -s format png /tmp/puppy.jpg --out tests/fixtures/llm_multimodal/sample_image.png
base64 -i tests/fixtures/llm_multimodal/sample_image.png -o tests/fixtures/llm_multimodal/sample_image_b64.txt

# 重新生成 PDF base64
pip3 install --user reportlab   # 若未安装；macOS 14+ 需加 --break-system-packages
python3 tests/fixtures/llm_multimodal/gen_sample_pdf.py > tests/fixtures/llm_multimodal/sample_pdf_b64.txt
```

## 注意事项

- `*_b64.txt` 是无前缀（不含 `data:image/png;base64,`）的纯 base64 字符串，wire
  拼装由 `OpenAiResponsesProvider::part_to_responses_value` 完成。
- 图片大小 < `IMAGE_MAX_BYTES` (4.5 MB)、PDF 大小 < `FILE_MAX_BYTES` (25 MB) — 见
  `core/llm/types.rs` 的 helper 校验。
