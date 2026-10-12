# Linktrix ASIO

Source code of the open-source ASIO components of **Linktrix Tools** (Windows x64).
Linktrix Tools uses the Steinberg ASIO SDK under its GPLv3 option; every piece
of code derived from the SDK is here. The application itself contains none and
talks to these components only through the interfaces documented below.

| Component | Crate | License | Ships as |
|---|---|---|---|
| IASIO interface (translated from the SDK) | `crates/linktrix-asio` | GPL-3.0-only | linked into the two below |
| Linktrix Audio 64 IO, the ASIO driver DAWs open | `crates/wat-asio` | GPL-3.0-only | `linktrix_audio_asio.dll` |
| Hardware ASIO host, one process per device | `crates/linktrix-asio-host` | GPL-3.0-only | `linktrix_asio_host.exe` |
| Shared-memory bus, helper protocol, 24-bit packing | `crates/linktrix-bus` | MIT | linked into the components above and into Linktrix Tools |

## How they connect

- `linktrix_audio_asio.dll` is an in-process COM ASIO driver
  (CLSID `{A874E189-2361-4C9E-AE64-56A474986402}`, registered as
  *Linktrix Audio 64 IO*). It runs inside the DAW and exchanges 64 input and
  64 output channels with Linktrix Tools through the shared-memory bus in
  `linktrix-bus` (`bus.rs`).
- `linktrix_asio_host.exe` hosts one hardware ASIO driver in its own process
  and exchanges audio blocks through shared memory and a text control channel.
  Any program may drive it: see [docs/ASIO-HELPER-PROTOCOL.md](docs/ASIO-HELPER-PROTOCOL.md).

## Build

Requires Windows x64 and Rust (MSVC toolchain).

```powershell
cargo build --release --locked
cargo test --locked
```

Outputs: `target\release\linktrix_audio_asio.dll` and
`target\release\linktrix_asio_host.exe`. Released binaries are additionally
Authenticode-signed by Linktrix; the signature is not part of the source.

`scripts\Test-AsioAbi.ps1` loads the built DLL through the unmodified Steinberg
C++ interface (`third_party\asio`) to check the ABI (needs Visual Studio C++
tools and the Windows SDK).

To register a self-built DLL for testing (administrator PowerShell), point the
COM registration at it:

```powershell
$clsid = '{A874E189-2361-4C9E-AE64-56A474986402}'
New-Item "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" -Force | Out-Null
Set-Item "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" -Value "$PWD\target\release\linktrix_audio_asio.dll"
New-ItemProperty "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" -Name ThreadingModel -Value Both -Force | Out-Null
New-Item 'HKLM:\SOFTWARE\ASIO\Linktrix Audio 64 IO' -Force | Out-Null
New-ItemProperty 'HKLM:\SOFTWARE\ASIO\Linktrix Audio 64 IO' -Name CLSID -Value $clsid -Force | Out-Null
```

## Versions

Tags match Linktrix Tools releases: tag `v0.29.1` is the corresponding source
of the `linktrix_audio_asio.dll` and `linktrix_asio_host.exe` shipped with
Linktrix Tools 0.29.1.

## License

- `crates/linktrix-asio`, `crates/wat-asio`, `crates/linktrix-asio-host`,
  `tests/asio-abi.cpp`, `scripts/`: GNU General Public License version 3 only
  ([LICENSE](LICENSE)).
- `crates/linktrix-bus`: MIT ([LICENSE-MIT](crates/linktrix-bus/LICENSE-MIT)).
- `third_party/asio`: Steinberg ASIO SDK interface headers, used under the
  GPLv3 option of their license ([LICENSE.txt](third_party/asio/LICENSE.txt)).

Copyright (c) 2026 Linktrix.

ASIO is a trademark and software of Steinberg Media Technologies GmbH. This
project is not affiliated with or endorsed by Steinberg.
