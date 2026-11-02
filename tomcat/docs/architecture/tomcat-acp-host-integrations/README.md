# Tomcat ACP 宿主接入资料

本目录按宿主分成两个**互相独立**的资料包。每个子目录都包含自己的源码、补丁、测试、配置示例、许可证和校验清单；不要把一个子目录的文件复制到另一个宿主。

- [Paperclip 接入包](./paperclip-tomcat-acp-integration/README.md)
- [Multica 接入包](./multica-tomcat-acp-integration/README.md)

两个资料包都基于标准 ACP 接入 Tomcat，但各自复用目标宿主自己的注册、执行、权限、工作目录、恢复和发布机制。公司 GitLab 二开仓库需要分别按对应包重新核对和验收。
