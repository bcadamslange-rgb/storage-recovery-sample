# Recovery decisions

The protocol is invented and the data is synthetic. The specification is the contract.

1. Enumerate only direct, no-follow regular files. Parse numeric segment identifiers and validate headers.
2. Scan each accepted file until the first invalid record. Validate CRC-32C for every record type. Never resynchronize past damage.
3. Validate compaction source ordering, completed brackets and transitive supersession before replay. Overlapping source sets without supersession are fatal.
4. Group transactions across retained files. An abort wins; otherwise exactly one commit is required.
5. Resolve each key by logical commit sequence and operation index. Identical coordinates deduplicate only when operation and value agree.
6. Emit sorted, canonical JSON. Values use standard padded base64. On semantic failure remove stale state and return status 2 with a diagnostic report.

The two output files are individually replaced through temporary-file renames. They are not a transactional pair, and the implementation does not fsync data or directory metadata. This exercise demonstrates recovery semantics, not guaranteed crash-durable output publication.

The caller must provide distinct writable output files outside the input tree. Invalid UTF-8 user keys are outside the specified input domain.
