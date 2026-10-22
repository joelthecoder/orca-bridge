import { Action, ActionPanel, List, Toast, getPreferenceValues, showToast } from "@raycast/api";
import { execFile } from "node:child_process";
import { homedir } from "node:os";
import { useEffect, useState } from "react";

type Session = { id: string; title: string; agent: string; cwd: string | null; model: string | null };
const binary = () => getPreferenceValues<{ binary?: string }>().binary || `${homedir()}/.local/bin/orca-bridge`;

export default function Search() {
  const [query, setQuery] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let active = true;
    setLoading(true);
    const child = execFile(binary(), ["search", query, "--limit", "30", "--json"], { maxBuffer: 8 * 1024 * 1024 }, (error, stdout) => {
      if (!active) return;
      setLoading(false);
      if (error) {
        setSessions([]);
        void showToast({ style: Toast.Style.Failure, title: "Orca search failed", message: error.message });
        return;
      }
      try { setSessions(JSON.parse(stdout) as Session[]); }
      catch { void showToast({ style: Toast.Style.Failure, title: "Invalid Orca Bridge output" }); }
    });
    return () => { active = false; child.kill(); };
  }, [query]);
  return <List isLoading={loading} onSearchTextChange={setQuery} searchBarPlaceholder="Search Orca sessions…" filtering={false}>
    {sessions.map((session) => <List.Item key={session.id} title={session.title || "Untitled session"} subtitle={session.cwd || undefined}
      accessories={[{ text: `${session.agent} · ${session.model || "unknown model"}` }]}
      actions={<ActionPanel><Action title="Open in Orca" onAction={async () => {
        execFile(binary(), ["open", session.id, "--json"], (error) => {
          if (error) void showToast({ style: Toast.Style.Failure, title: "Could not open session", message: error.message });
        });
      }} /><Action.CopyToClipboard title="Copy Session ID" content={session.id} /></ActionPanel>} />)}
  </List>;
}
