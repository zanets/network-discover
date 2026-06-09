# Hostname & Vendor 顯示不完整

**Date:** 2026-06-10
**Affected:** `src/discovery.rs`, `assets/oui.txt`

---

## Issue 1 — Vendor 幾乎全空白

### Core Concept
Bundled OUI database coverage

### Root Cause (ELI5)
`assets/oui.txt` 只有 21 筆手寫記錄（VMware、Raspberry Pi 等幾家），
而真實網路上的 MAC OUI 有將近 4 萬個登記廠商。
沒在表裡的 MAC 就查不到廠商名稱。

### Fix
從 IEEE Standards Registration Authority 下載完整 CSV，轉換成 tab-separated 格式重新產生 `assets/oui.txt`：

```bash
curl -s "https://standards-oui.ieee.org/oui/oui.csv" | python3 -c "
import csv, sys
print('# OUI vendor lookup')
print('# Format: AABBCC<tab>Vendor Name')
print('# Source: IEEE Standards Registration Authority')
print()
reader = csv.reader(sys.stdin)
next(reader)
for row in reader:
    if len(row) < 3: continue
    assignment, org_name = row[1].strip().upper(), row[2].strip()
    if len(assignment) == 6 and org_name:
        print(f'{assignment}\t{org_name}')
" > assets/oui.txt
# 結果：21 行 → 39,498 行
```

---

## Issue 2 — Hostname 幾乎全空白

### Core Concept
mDNS Unicast vs. Multicast — protocol misuse

### Root Cause (ELI5)
原本的 `probe_mdns` 把 service-type PTR query（`_workstation._tcp.local`、`_http._tcp.local`）
unicast 送到目標裝置的 port 5353。
但 mDNS 的 service discovery 是設計給 **multicast** (224.0.0.251) 用的，
裝置不會回應「你有沒有在跑這個服務？」這種 unicast 問法。
正確問法是：「你的反向 IP 名稱是什麼？」→ reverse PTR lookup。

### Fix

**`probe_mdns`** — 改成 reverse PTR unicast query：
```rust
// 之前（無效）
probe_mdns_type(ip, "_workstation._tcp.local").await
probe_mdns_type(ip, "_http._tcp.local").await

// 之後（正確）
let [a, b, c, d] = ip.octets();
let reverse_name = format!("{}.{}.{}.{}.in-addr.arpa", d, c, b, a);
// 送到 (ip, 5353)，解析 PTR rdata 取得 .local hostname
```

**`probe_nbns`** — 新增 NetBIOS Node Status Request (port 137) 補齊 Windows 機器：
```rust
// Node Status Request wildcard "*" encoded as NBT:
// '*' (0x2A) → 'C','K'; padding 0x00 → 'A','A' (×15)
socket.send_to(request, (ip, 137)).await;
// 解析 response 裡的 name table，找 suffix=0x00 (workstation) 且非 group flag
```

**`probe_friendly_name`** 優先順序更新：
```
mDNS reverse PTR → NetBIOS NBNS → SSDP UPnP
```
