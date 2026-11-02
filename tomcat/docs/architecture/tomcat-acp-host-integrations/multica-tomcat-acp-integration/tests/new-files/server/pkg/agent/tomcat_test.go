package agent

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The current test executable is the fake ACP peer; never discover a user's CLI.
func TestTomcatHelperProcess(t *testing.T) {
	mode := os.Getenv("MULTICA_TOMCAT_TEST_HELPER")
	if mode == "" {
		return
	}
	root := os.Getenv("TOMCAT_TEST_ROOT")
	if mode == "child" {
		for {
			f, _ := os.OpenFile(filepath.Join(root, "ticks"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
			if f != nil {
				_, _ = f.WriteString("tick\n")
				_ = f.Close()
			}
			time.Sleep(20 * time.Millisecond)
		}
	}
	trace, err := os.OpenFile(filepath.Join(root, "trace.jsonl"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		os.Exit(90)
	}
	enc := json.NewEncoder(os.Stdout)
	log := json.NewEncoder(trace)
	cwd, _ := os.Getwd()
	_ = log.Encode(map[string]any{"cwd": cwd, "args": os.Args, "marker": os.Getenv("TOMCAT_TEST_MARKER")})
	options := func(model string) []any {
		return []any{map[string]any{"id": "model", "currentValue": model, "type": "select", "options": []any{map[string]any{"value": model, "name": model}}}}
	}
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		var req struct {
			ID     int            `json:"id"`
			Method string         `json:"method"`
			Params map[string]any `json:"params"`
		}
		if json.Unmarshal(scanner.Bytes(), &req) != nil {
			os.Exit(91)
		}
		_ = log.Encode(req)
		response := map[string]any{"jsonrpc": "2.0", "id": req.ID}
		switch req.Method {
		case "initialize":
			response["result"] = map[string]any{"protocolVersion": 1, "agentCapabilities": map[string]any{"loadSession": true, "mcpCapabilities": map[string]any{"http": true, "sse": true}}}
		case "session/new":
			response["result"] = map[string]any{"sessionId": "tomcat-fixture", "configOptions": options("configured-model")}
		case "session/load":
			if mode == "missing" || mode == "busy" {
				message := "unknown_session"
				if mode == "busy" {
					message = "session_in_use"
				}
				response["error"] = map[string]any{"code": -32603, "message": message}
				break
			}
			_ = enc.Encode(map[string]any{"jsonrpc": "2.0", "method": "session/update", "params": map[string]any{"sessionId": "tomcat-fixture", "update": map[string]any{"sessionUpdate": "agent_message_chunk", "content": map[string]any{"type": "text", "text": "old history"}}}})
			response["result"] = map[string]any{"configOptions": options("configured-model")}
		case "session/set_config_option":
			if mode == "bad-model" {
				response["error"] = map[string]any{"code": -32602, "message": "unknown_model"}
			} else {
				response["result"] = map[string]any{"configOptions": options(fmt.Sprint(req.Params["value"]))}
			}
		case "session/prompt":
			_ = os.WriteFile(filepath.Join(root, "prompted"), []byte("yes"), 0600)
			if mode == "wait" || mode == "hang" {
				if mode == "hang" {
					self, _ := os.Executable()
					child := exec.Command(self, "-test.run=^TestTomcatHelperProcess$")
					child.Env = append(os.Environ(), "MULTICA_TOMCAT_TEST_HELPER=child")
					child.Stdout, child.Stderr = os.Stdout, os.Stderr
					if child.Start() != nil {
						os.Exit(92)
					}
				}
				continue
			}
			if mode == "failure" {
				response["error"] = map[string]any{"code": -32603, "message": "model unavailable"}
				break
			}
			_ = enc.Encode(map[string]any{"jsonrpc": "2.0", "method": "session/update", "params": map[string]any{"sessionId": "tomcat-fixture", "update": map[string]any{"sessionUpdate": "agent_message_chunk", "content": map[string]any{"type": "text", "text": "new answer"}}}})
			stop := "end_turn"
			if mode == "cancelled" {
				stop = "cancelled"
			}
			if mode == "invalid" {
				stop = ""
			}
			response["result"] = map[string]any{"stopReason": stop, "usage": map[string]any{"inputTokens": 5, "outputTokens": 3}}
		default:
			os.Exit(93)
		}
		_ = enc.Encode(response)
	}
	if mode == "hang" {
		for {
			time.Sleep(time.Hour)
		}
	}
	time.Sleep(100 * time.Millisecond)
	_ = os.WriteFile(filepath.Join(root, "stopped"), []byte("yes"), 0600)
	if mode == "shutdown-failure" {
		os.Exit(94)
	}
	os.Exit(0)
}

func tomcatTestBackend(t *testing.T, mode string) (Backend, string) {
	t.Helper()
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	backend, err := New("tomcat", Config{ExecutablePath: self, LaunchPrefix: []string{"-test.run=^TestTomcatHelperProcess$"}, Logger: slog.New(slog.NewTextHandler(io.Discard, nil)), Env: map[string]string{"MULTICA_TOMCAT_TEST_HELPER": mode, "TOMCAT_TEST_ROOT": root, "TOMCAT_TEST_MARKER": "host-only"}})
	if err != nil {
		t.Fatal(err)
	}
	return backend, root
}

func tomcatResult(t *testing.T, session *Session) Result {
	t.Helper()
	select {
	case result := <-session.Result:
		return result
	case <-time.After(tomcatShutdownGrace + 10*time.Second):
		t.Fatal("Tomcat backend did not return")
		return Result{}
	}
}

func TestTomcatExecutionAndResume(t *testing.T) {
	for _, resume := range []string{"", "tomcat-fixture"} {
		t.Run("resume="+resume, func(t *testing.T) {
			backend, root := tomcatTestBackend(t, "complete")
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			session, err := backend.Execute(ctx, "only user prompt", ExecOptions{Cwd: root, Model: "custom-model", ResumeSessionID: resume, SystemPrompt: "must-not-be-inlined", CustomArgs: []string{"--help"}, McpConfig: json.RawMessage(`{"mcpServers":{"host":{"type":"http","url":"http://example.test/mcp"}}}`)})
			if err != nil {
				t.Fatal(err)
			}
			result := tomcatResult(t, session)
			if result.Status != "completed" || result.SessionID != "tomcat-fixture" || result.Output != "new answer" {
				t.Fatalf("result: %+v", result)
			}
			if result.Usage["custom-model"].InputTokens != 5 {
				t.Fatalf("usage: %+v", result.Usage)
			}
			if _, err := os.Stat(filepath.Join(root, "stopped")); err != nil {
				t.Fatal("result arrived before peer cleanup", err)
			}
			if !session.TerminalObserved() {
				t.Fatal("missing terminal boundary")
			}
			var messages []Message
			for m := range session.Messages {
				messages = append(messages, m)
			}
			if len(messages) < 2 || messages[0].SessionID != "tomcat-fixture" || messages[0].Type != MessageStatus {
				t.Fatalf("missing early session pin: %+v", messages)
			}
			data, err := os.ReadFile(filepath.Join(root, "trace.jsonl"))
			if err != nil {
				t.Fatal(err)
			}
			text := string(data)
			for _, want := range []string{"host-only", root, "session/set_config_option", "custom-model", "http://example.test/mcp", "only user prompt"} {
				if !strings.Contains(text, want) {
					t.Errorf("trace missing %q: %s", want, text)
				}
			}
			if strings.Contains(text, "--help") || strings.Contains(text, "must-not-be-inlined") {
				t.Errorf("unexpected args/prompt: %s", text)
			}
			if resume != "" && (strings.Contains(text, "session/new") || !strings.Contains(text, "session/load")) {
				t.Errorf("resume forked: %s", text)
			}
		})
	}
}

func TestTomcatCompletedTurnSurvivesLateCancellation(t *testing.T) {
	backend, root := tomcatTestBackend(t, "complete")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	session, err := backend.Execute(ctx, "task", ExecOptions{Cwd: root})
	if err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for !session.TerminalObserved() {
		if time.Now().After(deadline) {
			t.Fatal("no terminal response")
		}
		time.Sleep(time.Millisecond)
	}
	cancel() // The protocol completed, but the peer is still cleaning up.
	result := tomcatResult(t, session)
	if result.Status != "completed" {
		t.Fatalf("late cancellation changed a completed turn: %+v", result)
	}
	if _, err := os.Stat(filepath.Join(root, "stopped")); err != nil {
		t.Fatal(err)
	}
}

func TestTomcatFailuresAndResumeClassification(t *testing.T) {
	for _, tc := range []struct {
		mode, status string
		rejected     bool
	}{{"missing", "failed", true}, {"busy", "failed", false}, {"failure", "failed", false}, {"cancelled", "aborted", false}, {"invalid", "failed", false}, {"shutdown-failure", "failed", false}, {"bad-model", "failed", false}} {
		t.Run(tc.mode, func(t *testing.T) {
			backend, root := tomcatTestBackend(t, tc.mode)
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			session, err := backend.Execute(ctx, "task", ExecOptions{Cwd: root, ResumeSessionID: "tomcat-fixture", Model: "custom-model"})
			if err != nil {
				t.Fatal(err)
			}
			result := tomcatResult(t, session)
			if result.Status != tc.status || result.ResumeRejected != tc.rejected || result.Error == "" {
				t.Fatalf("result: %+v", result)
			}
			if _, err := os.Stat(filepath.Join(root, "stopped")); err != nil {
				t.Fatal("result before shutdown", err)
			}
		})
	}
}

func TestTomcatCancellationWaitsForEOFExitAndKillsUnresponsiveTree(t *testing.T) {
	for _, mode := range []string{"wait", "hang"} {
		t.Run(mode, func(t *testing.T) {
			backend, root := tomcatTestBackend(t, mode)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			session, err := backend.Execute(ctx, "task", ExecOptions{Cwd: root})
			if err != nil {
				t.Fatal(err)
			}
			ready := "prompted"
			if mode == "hang" {
				ready = "ticks"
			}
			deadline := time.Now().Add(10 * time.Second)
			for {
				if _, err := os.Stat(filepath.Join(root, ready)); err == nil {
					break
				}
				if time.Now().After(deadline) {
					t.Fatal("peer did not become ready")
				}
				time.Sleep(10 * time.Millisecond)
			}
			started := time.Now()
			cancel()
			result := tomcatResult(t, session)
			if result.Status != "aborted" {
				t.Fatalf("result: %+v", result)
			}
			if mode == "wait" {
				if _, err := os.Stat(filepath.Join(root, "stopped")); err != nil {
					t.Fatal("peer was killed rather than allowed to finish", err)
				}
			} else {
				if time.Since(started) < tomcatShutdownGrace {
					t.Fatal("killed before grace expired")
				}
				if !strings.Contains(result.Error, "cleanup was not acknowledged") {
					t.Fatalf("dishonest cleanup result: %+v", result)
				}
				before, _ := os.ReadFile(filepath.Join(root, "ticks"))
				time.Sleep(120 * time.Millisecond)
				after, _ := os.ReadFile(filepath.Join(root, "ticks"))
				if string(before) != string(after) {
					t.Fatal("descendant still writing after result")
				}
			}
		})
	}
}

func TestTomcatRegistrationAndInputValidation(t *testing.T) {
	backend, err := New("tomcat", Config{ExecutablePath: filepath.Join(t.TempDir(), "missing")})
	if err != nil {
		t.Fatal(err)
	}
	for _, cwd := range []string{"", ".", filepath.Join(t.TempDir(), "missing")} {
		if _, err := backend.Execute(context.Background(), "task", ExecOptions{Cwd: cwd}); err == nil {
			t.Fatalf("accepted cwd %q", cwd)
		}
	}
	catalog, err := ListModels(context.Background(), "tomcat", NewCommand("must-not-run", nil))
	if err != nil || len(catalog.Models) != 0 || !ModelSelectionSupported("tomcat") {
		t.Fatalf("catalog=%+v err=%v", catalog, err)
	}
	if LaunchHeader("tomcat") != "tomcat acp" {
		t.Fatal("wrong launch header")
	}
	for _, tc := range []struct {
		version string
		valid   bool
	}{{"0.1.79", false}, {"0.1.80", true}, {"0.2.0", true}, {"not-a-version", false}} {
		if err := CheckMinVersion("tomcat", tc.version); (err == nil) != tc.valid {
			t.Errorf("version %s: %v", tc.version, err)
		}
	}
}
