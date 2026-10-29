# Engineering task

A storage process lost its in-memory key index during interrupted compaction. Reconstruct the durable key-value state from the remaining segment files.

Implement the command-line contract in SPEC.md. Handle torn records, cross-file transactions, logical write ordering, completed and abandoned compactions, and conflicting duplicate operations. Produce canonical state and diagnostic JSON without changing the inputs.

Acceptance is based on observed exit status, state and report contents, input preservation and repeatability. A correct answer must satisfy the complete specification; passing a few demonstration cases alone is insufficient.

The public portfolio includes the implementation and selected synthetic examples. It does not include any protected evaluation corpus or platform-specific task scaffold.
