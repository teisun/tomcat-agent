# Tomcat for VS Code

Tomcat brings the `tomcat serve --stdio` agent runtime into the VS Code Chat UI.
After installing the extension, you can talk to it with `@tomcat` in Chat.

## Prerequisites

Before using the extension, make sure:

1. The `tomcat` CLI is installed.
2. Running `tomcat --version` works in your shell.
3. If this is your first time using Tomcat, run `tomcat init` once to finish
   the runtime setup.

If `tomcat` is already on your `PATH`, you usually do **not** need to configure
`settings.json`.

## Install from VSIX

1. Build and package the extension:

   ```bash
   cd <repo>/tomcat-vscode-ext
   npm install
   npm run package:vsix
   ```

2. Install the generated VSIX:

   ```bash
   code --install-extension /path/to/tomcat-vscode-ext-0.1.1.vsix --force
   ```

3. Reload VS Code.

## First chat

Open the Chat view and send a message that starts with `@tomcat`:

```text
@tomcat help me understand this repository
```

Tomcat sessions are mapped to the current chat thread. Starting a new chat gives
you a new Tomcat session automatically.

## Commands

The extension contributes these commands:

- `Tomcat: Restart Serve`
- `Tomcat: Start New Session`
- `Tomcat: List Sessions`

## Optional settings

You only need these settings if the extension cannot discover `tomcat`
automatically or if you want to override the defaults:

```json
{
  "tomcat.path": "/absolute/path/to/tomcat",
  "tomcat.session.defaultCwd": "/absolute/path/to/workspace",
  "tomcat.serve.extraArgs": []
}
```

By default:

- `tomcat.path` falls back to `tomcat` and is auto-discovered from your shell
  environment when possible.
- `tomcat.session.defaultCwd` falls back to the first workspace folder.

## Troubleshooting

If `@tomcat` does not appear in Chat:

1. Make sure the extension is installed and enabled.
2. Reload the VS Code window.
3. Confirm that your VS Code version is compatible with the extension.

If the extension cannot start Tomcat:

1. Run `tomcat --version` in a terminal.
2. If that fails, fix your `PATH` or set `tomcat.path`.
3. If `tomcat` starts but chat requests fail later, run `tomcat init` and
   verify your runtime configuration.

If Tomcat exits during a conversation:

1. Run `Tomcat: Restart Serve`.
2. Check the `Tomcat` output channel for startup and stderr details.
