# Copy IP to Clipboard (TUI)

**Date:** 2026-06-10
**Affected file:** `src/tui.rs`
**Core Concept:** Clipboard integration in TUI app via `arboard` crate

## Root Cause (ELI5)
TUI 沒有內建複製到剪貼簿的功能，需要外部 crate (`arboard`) 操作系統剪貼簿。

## Fixed Code

```rust
// Cargo.toml
arboard = "3"

// src/tui.rs — new method on App
fn copy_ip(&mut self) {
    if let Some(idx) = self.table_state.selected() {
        if idx < self.hosts.len() {
            let ip = self.hosts[idx].ip.to_string();
            match Clipboard::new().and_then(|mut cb| cb.set_text(&ip)) {
                Ok(()) => self.wol_status = Some((format!("✓  Copied {ip}"), 25)),
                Err(_) => self.wol_status = Some(("✗  Clipboard unavailable".to_string(), 25)),
            }
        }
    }
}

// Key binding in Mode::HostList
KeyCode::Char('c') => app.copy_ip(),
```
