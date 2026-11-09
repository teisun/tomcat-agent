package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
	"testing/synctest"
	"time"

	"github.com/multica-ai/multica/server/internal/daemon/execenv"
	"github.com/multica-ai/multica/server/pkg/agent"
	"github.com/multica-ai/multica/server/pkg/taskfailure"
)

func TestTomcatDiscoveryHonorsOverrideAndModel(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX test executable")
	}
	home := t.TempDir()
	redirectTestHome(t, home)
	bin := filepath.Join(t.TempDir(), "tomcat")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\nprintf 'tomcat 0.1.80\\n'\n"), 0755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", "/usr/bin:/bin")
	t.Setenv("SHELL", "/bin/sh")
	t.Setenv("MULTICA_DAEMON_ID", "11111111-1111-1111-1111-111111111111")
	pinNonCodexAgentsToMissingPaths(t)
	t.Setenv("MULTICA_CODEX_PATH", filepath.Join(home, "missing-codex"))
	t.Setenv("MULTICA_TOMCAT_PATH", bin)
	t.Setenv("MULTICA_TOMCAT_MODEL", "  configured-model  ")
	cfg, err := LoadConfig(Overrides{ServerURL: "http://localhost:0", WorkspacesRoot: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	if entry, ok := cfg.Agents["tomcat"]; !ok || entry.Path != bin || entry.Command != bin || entry.Model != "configured-model" {
		t.Fatalf("Tomcat entry: %+v", cfg.Agents["tomcat"])
	}
	if providerNeedsInlineSystemPrompt("tomcat") {
		t.Fatal("Tomcat brief must stay in AGENTS.md")
	}
}

// Drive the real runner, backend process and filesystem defers: a cleanup
// failure must retain the agent's work and runtime brief in both directory modes.
func TestTomcatCleanupUnconfirmedPreservesLocalFiles(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX fixture executable")
	}
	for _, mode := range []string{localDirectoryModeInPlace, localDirectoryModeWorktree} {
		t.Run(mode, func(t *testing.T) {
			redirectTestHome(t, t.TempDir())
			tempBase := t.TempDir()
			t.Setenv("MULTICA_AGENT_TEMP_BASE", tempBase)
			repo := createDaemonTestRepo(t)
			bin := filepath.Join(t.TempDir(), "tomcat-fixture")
			if err := os.WriteFile(bin, []byte("#!/bin/sh\nprintf retained > retained.txt\nprintf retained > \"$TMPDIR/retained-temp\"\nprintf '%s' \"$TMPDIR\" > task-temp-path.txt\nexit 1\n"), 0755); err != nil {
				t.Fatal(err)
			}
			logger := slog.New(slog.NewTextHandler(io.Discard, nil))
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				_, _ = io.WriteString(w, `{}`)
			}))
			t.Cleanup(srv.Close)
			d := &Daemon{
				client: NewClient(srv.URL), logger: logger,
				workspaces: make(map[string]*workspaceState), activeEnvRoots: make(map[string]int),
				localPathLocks:     NewLocalPathLocker(),
				runtimeIndex:       map[string]Runtime{"rt": {ID: "rt", Provider: "tomcat", ProfileID: "profile"}},
				profileLaunchSpecs: map[string]profileLaunchSpec{"profile": {path: bin, version: "0.1.89"}},
				cfg:                Config{DaemonID: "fixture-daemon", WorkspacesRoot: t.TempDir(), AgentTimeout: 10 * time.Second, ServerBaseURL: srv.URL},
			}
			t.Cleanup(func() {
				d.activeEnvRootsMu.Lock()
				defer d.activeEnvRootsMu.Unlock()
				for _, lock := range d.unconfirmedTaskTempLocks {
					execenv.ReleaseTaskTempLock(lock)
				}
			})
			ref, err := json.Marshal(localDirectoryRef{LocalPath: repo, DaemonID: "fixture-daemon", ExecutionMode: mode})
			if err != nil {
				t.Fatal(err)
			}
			task := Task{ID: "cleanup-files", WorkspaceID: "ws", RuntimeID: "rt", IssueID: "issue", AgentID: "agent", AuthToken: "mat_fixture",
				Agent:            &AgentData{ID: "agent", Name: "fixture"},
				ProjectResources: []ProjectResourceData{{ID: "local", ResourceType: localDirectoryResourceType, ResourceRef: ref}}}
			result, runErr := d.runTask(t.Context(), task, "tomcat", 0, logger)
			if result.Status != "blocked" || result.FailureReason != "cleanup_unconfirmed" || result.WorkDir == "" {
				t.Fatalf("runTask: %+v, %v", result, runErr)
			}
			if content, err := os.ReadFile(filepath.Join(result.WorkDir, "retained.txt")); err != nil || string(content) != "retained" {
				t.Fatalf("agent work was removed: %q, %v", content, err)
			}
			if _, err := os.Stat(filepath.Join(result.WorkDir, "AGENTS.md")); err != nil {
				t.Fatalf("runtime brief was removed before cleanup acknowledgement: %v", err)
			}
			tempPath, err := os.ReadFile(filepath.Join(result.WorkDir, "task-temp-path.txt"))
			if err != nil {
				t.Fatal(err)
			}
			// Exercise the same lock-aware pruner used by another daemon, after
			// runTask has returned and unreferenced file handles could be finalized.
			runtime.GC()
			if removed, _ := execenv.PruneTaskTempDirs(tempBase, 0, time.Now(), logger); removed != 0 {
				t.Fatalf("GC removed %d unconfirmed task temp directories", removed)
			}
			if content, err := os.ReadFile(filepath.Join(string(tempPath), "retained-temp")); err != nil || string(content) != "retained" {
				t.Fatalf("task temp content was removed before cleanup acknowledgement: %q, %v", content, err)
			}
			var preserved *worktreePreservedError
			if mode == localDirectoryModeWorktree {
				if !errors.As(runErr, &preserved) || result.DurableWorkDir != "" || result.BranchName != "" {
					t.Fatalf("unconfirmed worktree was finalized: %+v, %v", result, runErr)
				}
			} else if runErr != nil {
				t.Fatal(runErr)
			}
		})
	}
}

// A fresh attempt can lose its session id before it confirms process cleanup.
// That uncertainty must beat the poisoned first result without reviving its id.
func TestTomcatFreshRetryPreservesCleanupUncertainty(t *testing.T) {
	first := agent.Result{Status: "failed", Error: "session not found", SessionID: "old-session", ResumeRejected: true}
	firstUsage := map[string]agent.TokenUsage{"model": {InputTokens: 5}}
	retry := agent.Result{Status: "cleanup_unconfirmed", Error: "Background processes may still be running", Usage: map[string]agent.TokenUsage{"model": {OutputTokens: 7}}}
	result, tools := reconcileFreshRetryResult(first, firstUsage, 0, retry, 1, nil)
	if result.Status != retry.Status || result.Error != retry.Error || result.SessionID != "" || tools != 1 {
		t.Fatalf("fresh cleanup uncertainty lost: %+v, tools=%d", result, tools)
	}
	if result.Usage["model"].InputTokens != 5 || result.Usage["model"].OutputTokens != 7 {
		t.Fatalf("attempt usage lost: %+v", result.Usage)
	}
	if shouldRetryWithFreshSession(result, "old-session", 0, "tomcat") {
		t.Fatal("cleanup uncertainty permitted another retry")
	}
}

// Reuse the accepted cleanup contract without importing Pi-specific capabilities.
type tomcatUnconfirmedCleanupBackend struct{ omitResult bool }

func (b tomcatUnconfirmedCleanupBackend) Execute(ctx context.Context, _ string, _ agent.ExecOptions) (*agent.Session, error) {
	messages := make(chan agent.Message, 1)
	results := make(chan agent.Result, 1)
	messages <- agent.Message{Type: agent.MessageToolUse, Tool: "bash", CallID: "t"}
	go func() {
		<-ctx.Done()
		close(messages)
		if !b.omitResult {
			results <- agent.Result{Status: "cleanup_unconfirmed", Error: "Background processes may still be running"}
		}
		close(results)
	}()
	return &agent.Session{Messages: messages, Result: results, CleanupRequired: true, TerminalObserved: func() bool { return false }}, nil
}

func TestTomcatCleanupUnconfirmedOutranksCancellation(t *testing.T) {
	for _, tc := range []struct {
		name            string
		manual, omitted bool
	}{
		{"watchdog/result", false, false}, {"watchdog/missing-result", false, true},
		{"manual/result", true, false}, {"manual/missing-result", true, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			d := newTestDaemon(t)
			d.cfg.AgentIdleWatchdog, d.cfg.AgentToolWatchdog = 50*time.Millisecond, 50*time.Millisecond
			ctx, cancel := context.WithCancel(t.Context())
			defer cancel()
			if tc.manual {
				timer := time.AfterFunc(30*time.Millisecond, cancel)
				defer timer.Stop()
			}
			result, _, err := d.executeAndDrain(ctx, tomcatUnconfirmedCleanupBackend{omitResult: tc.omitted}, "test", agent.ExecOptions{}, slog.Default(), "cleanup", "", new(atomic.Int32))
			if err != nil || result.Status != "cleanup_unconfirmed" || shouldRetryWithFreshSession(result, "existing-session", 0, "tomcat") {
				t.Fatalf("result=%+v err=%v", result, err)
			}
		})
	}
}

// A forwarder can need 35s for its bounded shared-service close. Missing the
// daemon's shorter handoff must preserve, not authorize, worktree cleanup.
type tomcatSlowCleanupBackend struct {
	gate   <-chan struct{}
	cancel context.CancelFunc
}

func (b tomcatSlowCleanupBackend) Execute(ctx context.Context, _ string, _ agent.ExecOptions) (*agent.Session, error) {
	messages := make(chan agent.Message)
	results := make(chan agent.Result, 1)
	go func() {
		<-ctx.Done()
		close(messages)
		<-b.gate
		time.Sleep(35 * time.Second)
		results <- agent.Result{Status: "aborted"}
		close(results)
	}()
	b.cancel()
	return &agent.Session{Messages: messages, Result: results, CleanupRequired: true, TerminalObserved: func() bool { return false }}, nil
}

func TestTomcatCleanupBeyondHandoffFailsClosed(t *testing.T) {
	d := newTestDaemon(t) // Keep the real HTTP fixture outside virtual time.
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		probe := &handoffProbe{gate: make(chan struct{}), prefix: "waiting for the backend to hand over its result after cancellation"}
		result, _, err := d.executeAndDrain(ctx, tomcatSlowCleanupBackend{gate: probe.gate, cancel: cancel}, "task", agent.ExecOptions{}, slog.New(probe), "tomcat-slow-cleanup", "", new(atomic.Int32))
		if !probe.seen.Load() || err != nil || result.Status != "cleanup_unconfirmed" || shouldRetryWithFreshSession(result, "existing-session", 0, "tomcat") {
			t.Fatalf("handoff=%v result=%+v err=%v", probe.seen.Load(), result, err)
		}
		time.Sleep(6 * time.Second)
		synctest.Wait()
	})
}

// Exercise both real handleTask acknowledgement paths, including parent
// cancellation and a preserved-worktree error. No database or real CLI is used.
func TestTomcatCleanupCancellationAcknowledgement(t *testing.T) {
	type acknowledgement struct {
		BranchName    string `json:"branch_name"`
		ErrorMessage  string `json:"error_message"`
		FailureReason string `json:"failure_reason"`
	}
	for _, poll := range []bool{true, false} {
		t.Run(map[bool]string{true: "poll", false: "post-run"}[poll], func(t *testing.T) {
			acks := make(chan acknowledgement, 1)
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				switch {
				case strings.HasSuffix(r.URL.Path, "/cancel-ack"):
					var ack acknowledgement
					if err := json.NewDecoder(r.Body).Decode(&ack); err != nil {
						t.Error(err)
					}
					acks <- ack
				case strings.HasSuffix(r.URL.Path, "/status"):
					w.Header().Set("Content-Type", "application/json")
					_, _ = io.WriteString(w, `{"status":"cancelled"}`)
				}
			}))
			t.Cleanup(srv.Close)
			d := &Daemon{
				client: NewClient(srv.URL), logger: slog.New(slog.NewTextHandler(io.Discard, nil)),
				workspaces:         make(map[string]*workspaceState),
				runtimeIndex:       map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "tomcat"}},
				cancelPollInterval: time.Hour,
			}
			if poll {
				d.cancelPollInterval = time.Millisecond
			}
			ctx, cancel := context.WithCancel(t.Context())
			defer cancel()
			d.runner = taskRunnerFunc(func(runCtx context.Context, _ Task, _ string, _ int, _ *slog.Logger) (TaskResult, error) {
				if poll {
					<-runCtx.Done()
				}
				cancel()
				return TaskResult{
					Status: "blocked", FailureReason: taskfailure.ReasonProcessCleanupUnconfirmed.String(),
					Comment: "unconfirmed process", BranchName: "work", WorkDir: "/fixture/preserved",
				}, &worktreePreservedError{err: errors.New("worktree retained at /fixture/preserved")}
			})
			d.handleTask(ctx, Task{ID: "cleanup-ack", RuntimeID: "rt-1", IssueID: "fixture-issue", Agent: &AgentData{Name: "fixture"}}, 0)
			select {
			case ack := <-acks:
				if ack.FailureReason != "cleanup_unconfirmed" || ack.BranchName != "work" || !strings.Contains(ack.ErrorMessage, "unconfirmed process") || !strings.Contains(ack.ErrorMessage, "/fixture/preserved") {
					t.Fatalf("cleanup diagnostics lost: %+v", ack)
				}
			default:
				t.Fatal("cancel acknowledgement was not delivered after parent cancellation")
			}
		})
	}
}
