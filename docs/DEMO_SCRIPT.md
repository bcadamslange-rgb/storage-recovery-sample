# Demonstration walkthrough

The MP4 and GIF visualize the actual result files produced by the included Rust executable. The display is paced for reading; it is not a live terminal recording.

1. Show the contract: rebuild state from retained segments and preserve the original files.
2. Show committed binary values: `AA==`, `AAA=` and `AAAA`; successful exit 0.
3. Show damaged DELETE handling: stop at its bad CRC, retain the earlier committed value and return 0.
4. Show a conflicting duplicate: return 2, emit a conflict report and remove stale state.
5. Show public-suite and negative-control outcomes. Give the reviewer the commands in README.md to reproduce them.

To record a live terminal version, open a terminal at this project root and run `bash scripts/demo.sh`. Use a clean window without personal paths or unrelated application content. Keep the terminal output readable and omit private environment information.
