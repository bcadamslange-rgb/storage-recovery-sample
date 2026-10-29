# Validation record

Executed on 28 October 2026 in Ubuntu on the Windows Subsystem for Linux, using rustc 1.85.0 and cargo 1.85.0. The portable toolchain was kept outside the portfolio. Cargo compiled the included source with `--release --locked --offline`.

| Check | Observed result |
|---|---|
| Public synthetic cases | 64/64 passed |
| Repeat identical successful recovery three times | State and report SHA-256 matched in all runs |
| Missing command-line arguments | Exit status 1 |
| Conflicting duplicate and invalid compaction source | Exit status 2, diagnostic report, stale state absent |
| Input preservation | File content, mode and modification time fingerprints unchanged in public cases |
| Existing internal synthetic corpus | 149/149 exact-output cases passed |
| Incorrect physical-order control | Rejected; failed 34 of 64 public cases |
| Incorrect DELETE/ABORT CRC-omission control | Rejected; failed 2 of 64 public cases |

The public cases include base64 padding for one, two and three zero bytes, Unicode/JSON escaping, malformed headers, record damage, cross-file transactions, abort and commit rules, complete and incomplete compactions, duplicate resolution, file classification and logical-order permutations. The 40 seeded cases vary order, not 40 unrelated behaviors.

The internal corpus is separate evidence, not a public test suite. No protected fixture files are distributed. Passing both suites is evidence for the stated contract, not a mathematical proof for arbitrary inputs.

Not executed: Docker image build/run, a native Windows Rust build, and any external assessment-platform verification.

The demonstration video is a paced replay of the recorded executable results. It is not a screen capture of a live terminal and does not claim real-time execution speed.

## Source fingerprints

```text
1065a4ffc3a1a2b3f0d8d555805286854ce493256d35e98c86e1eaa2d341d592  src/crc32c.rs
1f42351df0bafd05f5dd4396499dee24a7cb8be94b921d8ccb6d32025c3b5576  src/format.rs
fb4ba613174d4b32d4f11d74bfbf604be3967fcac5b87072a3fd648f56e67232  src/json.rs
07656c8b76cbccdc58e19da325accb3db27618279deae8ef9da88f687a5332f1  src/main.rs
5cedd44942cfbeacdfe2ad98116693c6482807500b85da60bc38eb87fba6a85f  src/recover.rs
```
