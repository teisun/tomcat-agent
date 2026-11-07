// Tomcat-side acceptance fixture, compiled only in a disposable copy of the
// pinned Multica module so Go's internal-package boundary remains intact.
// probe-runtimes exposes counts, not paths; call its unchanged LoadConfig to
// observe the actual resolved entry without adding a Multica product API.
package main

import (
	"encoding/json"
	"os"

	"github.com/multica-ai/multica/server/internal/daemon"
)

func main() {
	cfg, err := daemon.LoadConfig(daemon.Overrides{Profile: "cli-discovery", AllowNoAgents: true})
	if err != nil {
		panic(err)
	}
	entry, ok := cfg.Agents["tomcat"]
	if !ok {
		panic("pinned Multica did not discover Tomcat")
	}
	if err := json.NewEncoder(os.Stdout).Encode(struct {
		Path    string `json:"path"`
		Command string `json:"command"`
	}{entry.Path, entry.Command}); err != nil {
		panic(err)
	}
}
