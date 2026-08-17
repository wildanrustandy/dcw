# dcw — Dev-Captain Worktree (macOS)

Manajemen **git worktree + port + server Django HMX** dalam satu command — di-porting dari
`dev-captain` / `dcw` HashMicro DevSpace agar bisa dipakai di Mac lokal.

Setiap feature branch mendapat:

- **Worktree** sendiri di `<repo>/.worktrees/<branch>`
- **Port** sendiri (base `5201`, worktree `5202+`)
- **Database clone** sendiri (`hmxmain` → DB per-branch, via `createdb -T`, fallback dump/restore)
- **File privat** tersalin otomatis: `.agents`, `.claude`, `.vscode`, `AGENTS.md`, `hmx/.env`, `hmx/hmx/db.json`
- `.env` dan `db.json` worktree otomatis diarahkan ke DB branch tersebut (data utama aman)

## Install

```sh
git clone <repo-ini> ~/dcw
~/dcw/install.sh
```

Installer menyalin `dcw` ke `~/bin` (ikut PATH) dan mengecek dependency
(`git`, `python3`, `lsof`, PostgreSQL client tools).

### Prasyarat

1. **Repo HMX** ada di `~/dev/hmx-002` (default; bisa di-override)
2. **Venv Python** di `~/dev/.venv` — berisi dependency `requirements.txt` + `hmx` CLI
3. **PostgreSQL** lokal dengan DB utama `hmxmain` (client tools di PATH:
   `psql`, `createdb`, `dropdb`, `pg_dump` — `brew install postgresql@16 libpq`)
4. Kredensial DB dibaca otomatis dari `<repo>/hmx/.env` (`DATABASE_*`)

Kalau lokasi Anda beda, cukup set env sebelum memanggil:

```sh
export DEV_CAPTAIN_REPO="$HOME/dev/hmx-002"   # lokasi repo
export DEV_CAPTAIN_VENV="$HOME/dev/.venv"     # lokasi venv
export PORT_BASE=5201 PORT_MAX=5220           # rentang port
```

## Usage

```sh
dcw start <branch>    # buat worktree + clone DB + salin file privat + jalankan server
dcw status            # daftar captain aktif (port, pid, hidup/mati)
dcw list              # daftar semua git worktree
dcw logs <branch>     # tail log server
dcw stop <branch>     # stop server (worktree + DB tetap)
dcw abort <branch>    # hapus worktree + stop server + DROP DB branch (konfirmasi)
```

Contoh:

```sh
dcw start wildantest
# ✓ captain started: wildantest
#   worktree: ~/dev/hmx-002/.worktrees/wildantest
#   port:     5202   preview: http://127.0.0.1:5202
#   db:       cloning hmxmain -> wildantest

dcw start Inventory   # base branch → langsung di repo utama, port 5201
```

Base branch dideteksi dari branch yang aktif di repo (fallback `origin/HEAD`).

## State & kebersihan

| Path | Isi |
|---|---|
| `~/.dev-captain/ports.json` | registry port ↔ branch ↔ pid |
| `~/.dev-captain/logs/` | log server per branch |
| `<repo>/.worktrees/<branch>` | worktree (di-gitignore repo) |
| Postgres | DB per-branch (nama = nama branch, lowercase) |

File privat di worktree otomatis ditambahkan ke git exclude worktree — tidak akan ter-commit.

## Catatan

- `dcw abort` menghapus DB branch secara permanen (ada konfirmasi).
- DB `hmxmain` (utama) tidak pernah disentuh oleh `start`/`abort` worktree.
- Dirancang untuk macOS (`lsof`, `mkdir` lock; tanpa `fuser`/`ss`/`flock`).
