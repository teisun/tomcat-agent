# Paperclip 资料包校验记录

本文件只校验资料包自身，不代替公司 Paperclip 分支验收。

## 来源

- Paperclip 基线：`2f6ad2226aec330251438c47cbc45fd92e0d3132`
- Paperclip 接入提交：`1f010b1655b85ec4cfcf4ca45114e8fa899a601c`
- Tomcat 依赖代码：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`

## 校验方法

```sh
python3 - <<'PY'
import hashlib, json
from pathlib import Path
root = Path('tomcat/docs/architecture/tomcat-acp-host-integrations/paperclip-tomcat-acp-integration')
m = json.loads((root/'manifest.json').read_text())['host']
assert m['name'] == 'paperclip'
assert len(m['files']) == 46
for row in m['files']:
    p = root / row['delivery']
    assert p.is_file(), p
    if row['status'] == 'A':
        assert hashlib.sha256(p.read_bytes()).hexdigest() == row['sha256_after_integration'], p
for row in m['patches'] + m['licenses']:
    p = root / row['path']
    assert hashlib.sha256(p.read_bytes()).hexdigest() == row['sha256'], p
print('Paperclip package manifest and SHA-256: PASS')
PY
```

校验通过只说明新增文件、补丁、许可证和清单彼此一致；没有运行 Paperclip、Tomcat、模型或数据库，也没有访问公司 GitLab。

## 未包含

真实账号、API Key、公司仓库地址、数据库、node_modules、编译产物、临时 HOME、宿主运行日志和付费模型调用均未打包。
