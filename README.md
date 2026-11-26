# gridpnt

A terminal-native database tool. Keyboard only. One binary.

Early development. Today it opens **SQLite** files and **Databricks**
workspaces. Everything is read-only.

```
cargo build --release
./target/release/gridpnt some.db      # a SQLite file
./target/release/gridpnt work         # a connection you defined
```

## Connections

Define them in `~/.config/gridpnt/connections.ini`
(`%APPDATA%\gridpnt\connections.ini` on Windows):

```ini
[work]
type = databricks
profile = DEFAULT              ; a profile in ~/.databrickscfg, or: host = adb-123.azuredatabricks.net
warehouse = 1234567890abcdef   ; SQL warehouse id
env = prod                     ; shown in the status line
pin = main.sales, main.finance ; the only parts of the catalog to list

[scratch]
type = sqlite
path = /home/me/scratch.db
```

gridpnt stores no credentials. For Databricks it uses, in order: the
`token` in your `.databrickscfg` profile, `DATABRICKS_TOKEN`, or the
Databricks CLI (`databricks auth login --host ...` once, then gridpnt asks
the CLI for a token when it needs one).

Certificates are checked against your operating system's trust store, so a
corporate root certificate works with no setup. `HTTPS_PROXY` is honoured.

### What to know about Databricks

- **Pin what you use.** With `pin`, gridpnt lists only those catalogs and
  schemas and never crawls the rest.
- **Browsing is free.** The catalog comes from Unity Catalog, not from a
  warehouse. Only running a query can start a warehouse, and gridpnt warns
  you once before it would.
- **The catalog is remembered** between runs. Press `R` on a schema or
  table to re-read it.
- **Cancel means cancel.** `Ctrl-c`, and the 10-minute timeout, cancel the
  statement on the warehouse.
- **Read-only is gridpnt's rule, not Databricks'.** gridpnt refuses
  anything but a single read statement, but Databricks has no read-only
  session to back that up. Use a token that can only read what you intend.

## Run SQL from your editor

Open gridpnt in one terminal and send SQL from your editor or another terminal.

```
echo 'select 1' | gridpnt run
gridpnt run query.sql
gridpnt run --to work query.sql       # choose a connection by its displayed name
gridpnt run --to pid:12345 query.sql  # choose a particular running copy
```

Press `?` in gridpnt to see the ID for that run. If a connection name itself
starts with `pid:` or `name:`, prefix it with `name:` to use it literally.
When several copies match, `gridpnt run` lists their `--to` choices and
exits without sending SQL. Inside tmux it first checks the caller's window
and sends to a matching copy there if there is exactly one.

For Neovim:

```lua
-- run the paragraph under the cursor
vim.keymap.set("n", "<leader>r", "vip:w !gridpnt run<CR>")
-- run the selected lines
vim.keymap.set("x", "<leader>r", ":w !gridpnt run<CR>")
```

## Keys

Press `?` inside gridpnt for the full list.

| Keys | |
|---|---|
| `j` `k` `h` `l` | move |
| `gg` `G` `Ctrl-d` `Ctrl-u` | first / last row, half page |
| `0` `$` | first / last column |
| `/` `n` `N` | search the focused pane |
| `l` / `h` in the catalog | open / close |
| `Enter` | preview a table; show a whole cell |
| `y` `Y` | copy cell or name / row |
| `R` | re-read part of the catalog |
| `Tab` or `Ctrl-w` `h`/`l` | switch pane |
| `:` | run a query |
| `Ctrl-c` | cancel a running query |
| `q` | quit |

Copying uses the terminal's clipboard (OSC 52), so it works over SSH.
Inside tmux, add `set -g set-clipboard on` to your tmux config.

A [datapnt.com](https://datapnt.com) project.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
