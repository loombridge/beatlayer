# Library scripts

A script here makes or converts one kind of library item, run as
`python3 -I <script> <seed> <out file> [<input file>]` by `cargo xtask library`:
it writes the item's file to `<out file>`, the same bytes for the same seed
and input. The manifest pins each script by its SHA-256.
