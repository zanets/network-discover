# network-discover

A Rust CLI tool that discovers live hosts on a LAN.

## What it does

Scans a local subnet and reports active devices with their IP, MAC address, and optionally hostname and vendor name.

## Design decisions

### Scan method
- **Primary:** ARP — most reliable on LAN, directly yields MAC addresses
- **Fallback:** ICMP ping — used when ARP gets no response

### Target specification
- **Default:** Auto-detect all local network interfaces and their subnets
- **Override:** `--target <CIDR>` (e.g. `--target 192.168.1.0/24`)

### Output
- **Default:** Pretty table printed to stdout
- **`--output json`:** Machine-readable JSON for piping to other tools

### Hostname resolution
- **Default:** Off
- **`--resolve`:** Enables reverse DNS lookup per discovered host (opt-in because it slows scanning)

### Concurrency
- Uses `tokio` async runtime
- Default concurrency: 256 simultaneous probes
- **`--concurrency <N>`:** Override the limit

### Vendor lookup
- Resolves MAC OUI (first 3 bytes) to manufacturer name (e.g. `Apple`, `Raspberry Pi Foundation`)
- OUI database bundled into the binary at compile time — no network required at runtime

### Privilege handling
- ARP and raw ICMP require root / `cap_net_raw`
- On startup, detect if running without required privileges and fail fast with a clear message:
  ```
  Error: raw socket requires root. Run with sudo or grant cap_net_raw.
  ```

## Agent skills

### Issue tracker

Issues live in GitHub Issues. See `docs/agents/issue-tracker.md`.

### Triage labels

Default label vocabulary (needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context repo — one `CONTEXT.md` + `docs/adr/` at root. See `docs/agents/domain.md`.

## CLI interface

```
network-discover [OPTIONS]

Options:
  --target <CIDR>        Subnet to scan (default: auto-detect)
  --output <FORMAT>      Output format: table (default), json
  --resolve              Enable reverse DNS hostname lookup
  --concurrency <N>      Max concurrent probes (default: 256)
  -h, --help             Print help
  -V, --version          Print version
```
