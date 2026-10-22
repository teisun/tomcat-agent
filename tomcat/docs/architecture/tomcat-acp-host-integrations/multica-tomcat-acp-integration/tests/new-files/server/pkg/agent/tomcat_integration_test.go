//go:build agentintegration && !windows

package agent

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

// This is an opt-in cross-repository protocol test, not an authenticated model
// smoke test. The explicit binary runs against a local HTTP fixture and a fresh HOME.
func TestTomcatSharedServeWithHermesClient(t *testing.T) {
	requireRealAgentSmoke(t)
	binary := os.Getenv("MULTICA_TOMCAT_TEST_BIN")
	if !filepath.IsAbs(binary) {
		t.Fatal("MULTICA_TOMCAT_TEST_BIN must name an explicitly built Tomcat binary")
	}
	home, err := os.MkdirTemp("/tmp", "tc-m1-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(home) })
	workspace := filepath.Join(home, "work")
	if err := os.MkdirAll(workspace, 0700); err != nil {
		t.Fatal(err)
	}
	env := []string{"HOME=" + home, "PATH=" + os.Getenv("PATH"), "SHELL=/bin/sh", "NO_PROXY=127.0.0.1,localhost", "TOMCAT__UI__LANGUAGE=en"}
	command := func(ctx context.Context, args ...string) *exec.Cmd {
		cmd := exec.CommandContext(ctx, binary, args...)
		cmd.Env, cmd.Dir = env, workspace
		return cmd
	}
	run := func(args ...string) []byte {
		t.Helper()
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		out, err := command(ctx, args...).CombinedOutput()
		if err != nil {
			t.Fatalf("tomcat %v: %v\n%s", args, err, out)
		}
		return out
	}
	write := func(path, content string) {
		t.Helper()
		if err := os.WriteFile(path, []byte(content), 0600); err != nil {
			t.Fatal(err)
		}
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body map[string]any
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Error(err)
			return
		}
		raw, _ := json.Marshal(body)
		text := string(raw)
		if body["stream"] == false {
			w.Header().Set("Content-Type", "application/json")
			_ = json.NewEncoder(w).Encode(map[string]any{"choices": []any{map[string]any{"message": map[string]any{"role": "assistant", "content": "Fixture title"}, "finish_reason": "stop"}}})
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		send := func(delta map[string]any, finish any) {
			data, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{"index": 0, "delta": delta, "finish_reason": finish}}})
			_, _ = fmt.Fprintf(w, "data: %s\n\n", data)
			w.(http.Flusher).Flush()
		}
		tool := func(id, name string, args map[string]any) {
			arguments, _ := json.Marshal(args)
			send(map[string]any{"tool_calls": []any{map[string]any{"index": 0, "id": id, "type": "function", "function": map[string]any{"name": name, "arguments": string(arguments)}}}}, nil)
			send(map[string]any{}, "tool_calls")
		}
		hasResult := func(id string) bool {
			messages, _ := body["messages"].([]any)
			for _, value := range messages {
				message, _ := value.(map[string]any)
				if message["role"] == "tool" && message["tool_call_id"] == id {
					return true
				}
			}
			return false
		}
		switch {
		case strings.Contains(text, "Generate a short chat title"):
			send(map[string]any{"content": "Fixture title"}, "stop")
		case strings.Contains(text, "resume-fixture"):
			send(map[string]any{"content": "resumed-fixture"}, "stop")
		case strings.Contains(text, "start-background-fixture"):
			if hasResult("m1-shell") {
				// Keep the real model request active until Tomcat cancels it.
				<-r.Context().Done()
				return
			}
			tool("m1-shell", "bash", map[string]any{"command": "echo $$ > job.pid; while :; do printf tick >> ticks; sleep 0.05; done", "run_in_background": true})
		default:
			if !strings.Contains(text, "fixture-project-instruction") {
				t.Error("project AGENTS.md did not reach the model")
			}
			if hasResult("m1-write") {
				send(map[string]any{"content": "hermes-stream"}, "stop")
			} else {
				tool("m1-write", "write", map[string]any{"path": "completed.txt", "content": "from-hermes"})
			}
		}
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	t.Cleanup(server.Close)
	run("init")
	write(filepath.Join(home, ".tomcat", "mcp.json"), `{"mcpServers":{}}`)
	write(filepath.Join(home, ".tomcat", "tomcat.config.toml"), fmt.Sprintf("[llm]\ndefault_model = \"mock\"\ntitle_model = \"mock\"\n[context]\ncompaction_model = \"mock\"\n[skills]\nenabled = false\n[workspace]\nworkspace_roots = [%s]\n", strconv.Quote(workspace)))
	write(filepath.Join(home, ".tomcat", "models.toml"), fmt.Sprintf("[[models]]\nid = \"mock\"\napi = \"openai\"\nprovider = \"openai\"\nbase_url = %s\ncapabilities = { tools = true, vision = false, files = false, reasoning = false, web_search = false }\n", strconv.Quote(server.URL)))
	write(filepath.Join(workspace, "AGENTS.md"), "fixture-project-instruction\n")
	run("model", "key", "set", "openai", "local-fixture-key")
	t.Cleanup(func() { run("serve", "stop") })
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	if serviceBinary := os.Getenv("MULTICA_TOMCAT_TEST_SERVICE_BIN"); serviceBinary != "" {
		if !filepath.IsAbs(serviceBinary) {
			t.Fatal("MULTICA_TOMCAT_TEST_SERVICE_BIN must name another explicitly built CLI")
		}
		other := exec.CommandContext(ctx, serviceBinary, "serve", "start")
		other.Env, other.Dir = env, workspace
		out, err := other.CombinedOutput()
		if err != nil {
			t.Fatalf("start actual other service: %v\n%s", err, out)
		}
		var info map[string]any
		if err := json.Unmarshal(out, &info); err != nil {
			t.Fatal(err)
		}
		clientVersion := strings.TrimSpace(string(run("--version")))
		if info["serverVersion"] == strings.TrimPrefix(clientVersion, "tomcat ") {
			t.Fatal("cross-version acceptance requires two real different CLI versions")
		}
		t.Logf("cross-version backend: client=%s service=%v instance=%v", clientVersion, info["serverVersion"], info["instanceId"])
	}

	type peer struct {
		client *hermesClient
		stdin  io.WriteCloser
		done   <-chan error
		mu     sync.Mutex
		text   strings.Builder
	}
	open := func() *peer {
		t.Helper()
		cmd := command(ctx, "acp")
		stdin, err := cmd.StdinPipe()
		if err != nil {
			t.Fatal(err)
		}
		stdout, err := cmd.StdoutPipe()
		if err != nil {
			t.Fatal(err)
		}
		var stderr bytes.Buffer
		cmd.Stderr = &stderr
		p := &peer{stdin: stdin}
		p.client = &hermesClient{cfg: Config{Logger: slog.New(slog.NewTextHandler(io.Discard, nil))}, stdin: stdin, pending: make(map[int]*pendingRPC), onMessage: func(message Message) {
			if message.Type == MessageText {
				p.mu.Lock()
				p.text.WriteString(message.Content)
				p.mu.Unlock()
			}
		}}
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = stdin.Close(); _ = cmd.Process.Kill() })
		done := make(chan error, 1)
		p.done = done
		go func() {
			scanner := bufio.NewScanner(stdout)
			scanner.Buffer(make([]byte, 4096), 8*1024*1024)
			for scanner.Scan() {
				if !json.Valid(scanner.Bytes()) {
					t.Errorf("non-ACP stdout: %q", scanner.Text())
				}
				p.client.handleLine(scanner.Text())
			}
			p.client.closeAllPending(io.EOF)
			waitErr := cmd.Wait()
			if scanner.Err() != nil {
				waitErr = scanner.Err()
			}
			if waitErr != nil {
				waitErr = fmt.Errorf("%w: %s", waitErr, stderr.String())
			}
			done <- waitErr
		}()
		return p
	}
	request := func(p *peer, method string, params any) json.RawMessage {
		t.Helper()
		result, err := p.client.request(ctx, method, params)
		if err != nil {
			t.Fatalf("%s: %v", method, err)
		}
		return result
	}
	initialize := func(p *peer) {
		request(p, "initialize", map[string]any{"protocolVersion": 1, "clientInfo": map[string]any{"name": "multica-agent-sdk", "version": "0.2.0"}, "clientCapabilities": map[string]any{}})
	}
	closePeer := func(p *peer) {
		t.Helper()
		if err := p.stdin.Close(); err != nil {
			t.Fatal(err)
		}
		select {
		case err := <-p.done:
			if err != nil {
				t.Fatal(err)
			}
		case <-time.After(15 * time.Second):
			t.Fatal("forwarder did not finish graceful cleanup")
		}
	}
	p := open()
	initialize(p)
	created := request(p, "session/new", map[string]any{"cwd": workspace, "mcpServers": []any{}})
	sessionID := extractACPSessionID(created)
	if sessionID == "" {
		t.Fatal("session/new returned no session id")
	}
	prompt := func(text string) map[string]any {
		return map[string]any{"sessionId": sessionID, "prompt": []any{map[string]any{"type": "text", "text": text}}}
	}
	completed := request(p, "session/prompt", prompt("make-fixture-file"))
	if !bytes.Contains(completed, []byte(`"end_turn"`)) {
		t.Fatalf("terminal: %s", completed)
	}
	content, err := os.ReadFile(filepath.Join(workspace, "completed.txt"))
	if err != nil || string(content) != "from-hermes" {
		t.Fatalf("real file tool: %q, %v", content, err)
	}
	p.mu.Lock()
	streamed := p.text.String()
	p.mu.Unlock()
	if !strings.Contains(streamed, "hermes-stream") {
		t.Fatalf("hermesClient missed stream: %q", streamed)
	}

	promptDone := make(chan error, 1)
	go func() {
		_, err := p.client.request(ctx, "session/prompt", prompt("start-background-fixture"))
		promptDone <- err
	}()
	pidPath := filepath.Join(workspace, "job.pid")
	var pidBytes []byte
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		pidBytes, err = os.ReadFile(pidPath)
		if err == nil && len(pidBytes) > 0 {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(pidBytes)))
	if err != nil || pid <= 0 {
		t.Fatalf("background shell did not start: %q %v", pidBytes, err)
	}
	closePeer(p)
	if syscall.Kill(pid, 0) == nil {
		t.Fatal("forwarder exited while its shell was still alive")
	}
	<-promptDone
	before, err := os.ReadFile(filepath.Join(workspace, "ticks"))
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(150 * time.Millisecond)
	after, err := os.ReadFile(filepath.Join(workspace, "ticks"))
	if err != nil || !bytes.Equal(before, after) {
		t.Fatal("workspace changed after forwarder exit")
	}
	status := run("serve", "status")
	var state map[string]any
	if err := json.Unmarshal(status, &state); err != nil || state["running"] != true {
		t.Fatalf("shared service stopped: %s %v", status, err)
	}

	next := open()
	initialize(next)
	request(next, "session/load", map[string]any{"sessionId": sessionID, "cwd": workspace, "mcpServers": []any{}})
	resumed := request(next, "session/prompt", prompt("resume-fixture"))
	if !bytes.Contains(resumed, []byte(`"end_turn"`)) {
		t.Fatalf("resume terminal: %s", resumed)
	}
	next.mu.Lock()
	replay := next.text.String()
	next.mu.Unlock()
	if !strings.Contains(replay, "hermes-stream") || !strings.Contains(replay, "resumed-fixture") {
		t.Fatalf("saved history or resumed output missing: %q", replay)
	}
	closePeer(next)

	t.Run("registered_backend_execute_cancel_resume", func(t *testing.T) {
		work := filepath.Join(workspace, "backend")
		if err := os.MkdirAll(work, 0700); err != nil {
			t.Fatal(err)
		}
		write(filepath.Join(work, "AGENTS.md"), "fixture-project-instruction\n")
		environment := map[string]string{}
		// Simulate a separate host instead of inheriting the active Tomcat
		// session running this test. Only the isolated fixture's env is kept.
		for _, entry := range os.Environ() {
			key, _, _ := strings.Cut(entry, "=")
			environment[key] = ""
		}
		for _, entry := range env {
			key, value, _ := strings.Cut(entry, "=")
			environment[key] = value
		}
		backend, err := New("tomcat", Config{ExecutablePath: binary, Env: environment, Logger: slog.New(slog.NewTextHandler(io.Discard, nil))})
		if err != nil {
			t.Fatal(err)
		}
		execute := func(ctx context.Context, text, id string) *Session {
			s, err := backend.Execute(ctx, text, ExecOptions{Cwd: work, Model: "mock", ResumeSessionID: id})
			if err != nil {
				t.Fatal(err)
			}
			return s
		}
		first := tomcatResult(t, execute(ctx, "make-fixture-file", ""))
		if first.Status != "completed" || first.SessionID == "" {
			t.Fatalf("execute: %+v", first)
		}
		content, err := os.ReadFile(filepath.Join(work, "completed.txt"))
		if err != nil || string(content) != "from-hermes" {
			t.Fatalf("actual file tool: %q %v", content, err)
		}
		stopCtx, stop := context.WithCancel(ctx)
		defer stop()
		active := execute(stopCtx, "start-background-fixture", first.SessionID)
		deadline := time.Now().Add(20 * time.Second)
		var pid int
		for time.Now().Before(deadline) {
			data, _ := os.ReadFile(filepath.Join(work, "job.pid"))
			pid, _ = strconv.Atoi(strings.TrimSpace(string(data)))
			if pid > 0 {
				break
			}
			time.Sleep(20 * time.Millisecond)
		}
		if pid <= 0 {
			t.Fatal("backend shell never started")
		}
		stop()
		stopped := tomcatResult(t, active)
		if stopped.Status != "aborted" || stopped.SessionID != first.SessionID {
			t.Fatalf("cancel: %+v", stopped)
		}
		if syscall.Kill(pid, 0) == nil {
			t.Fatal("Result published before the shared service stopped its shell")
		}
		before, _ := os.ReadFile(filepath.Join(work, "ticks"))
		time.Sleep(150 * time.Millisecond)
		after, _ := os.ReadFile(filepath.Join(work, "ticks"))
		if !bytes.Equal(before, after) {
			t.Fatal("workspace changed after backend result")
		}
		last := tomcatResult(t, execute(ctx, "resume-fixture", first.SessionID))
		if last.Status != "completed" || last.SessionID != first.SessionID || last.Output != "resumed-fixture" {
			t.Fatalf("resume: %+v", last)
		}
		if err := json.Unmarshal(run("serve", "status"), &state); err != nil || state["running"] != true {
			t.Fatal("backend teardown stopped the shared service")
		}
	})
}
