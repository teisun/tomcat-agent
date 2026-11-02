package daemon

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"
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
	if entry, ok := cfg.Agents["tomcat"]; !ok || entry.Path != canonicalExecutablePath(bin) || entry.Model != "configured-model" {
		t.Fatalf("Tomcat entry: %+v", cfg.Agents["tomcat"])
	}
	if providerNeedsInlineSystemPrompt("tomcat") {
		t.Fatal("Tomcat brief must stay in AGENTS.md")
	}
}
