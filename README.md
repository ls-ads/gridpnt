# gridpnt

A terminal-native database tool. Keyboard only. One binary.

Early development. Today it opens SQLite files, read-only.

```
cargo build --release
./target/release/gridpnt some.db
```

| Keys | |
|---|---|
| `j` `k` `h` `l` | move |
| `gg` `G` `Ctrl-d` `Ctrl-u` | first / last row, half page |
| `0` `$` | first / last column |
| `l` / `h` in the catalog | open / close |
| `Enter` | preview table |
| `Tab` or `Ctrl-w` `h`/`l` | switch pane |
| `:` | run a query |
| `Ctrl-c` | cancel a running query |
| `q` | quit |

A [datapnt.com](https://datapnt.com) project.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
