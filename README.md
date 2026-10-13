# dcw — Dev-Captain Worktree (macOS)

Manajemen **git worktree + port + server Django HMX + Celery** dalam satu command — di-porting dari
`dev-captain` / `dcw` HashMicro DevSpace agar bisa dipakai di Mac lokal.

Setiap feature branch mendapat:

- **Worktree** sendiri di `<repo>/.worktrees/<branch>`
- **Port** sendiri (base `5200`, worktree `5201+`)
- **Database clone** sendiri (`hmxmain` → DB per-branch, via `createdb -T`, fallback dump/restore)
- **Namespace Redis** sendiri (`HMX_INSTANCE_NAMESPACE` per branch) — queue, hasil, cache, dan
  realtime tidak bercampur dengan branch lain, tanpa perlu Redis DB index tambahan
- **Worker Celery** sendiri, dikelola `start`/`stop`/`abort` (lihat [Celery per branch](#celery-per-branch))
- **File privat** tersalin otomatis: `.agents`, `.claude`, `.vscode`, `AGENTS.md`, `hmx/.env`, `hmx/hmx/db.json`
- `.env` dan `db.json` worktree otomatis diarahkan ke DB + namespace branch tersebut (data utama aman)

## Install

```sh
git clone <repo-ini> ~/dcw
~/dcw/install.sh
```

Installer **interaktif** — akan menanyakan:

1. Folder project HMX (default `~/dev/hmx-002`)
2. Folder venv Python (default `~/dev/.venv`)
3. Base port & max port (default `5200` & `5220` — base branch on `5200`, worktrees use `5201+`)

Jawaban tersimpan di `~/.config/dcw/config` dan dibaca `dcw` setiap kali jalan.
Bisa juga non-interaktif lewat flag:

```sh
~/dcw/install.sh --repo ~/work/hmx --venv ~/work/venv --port-base 5300 --port-max 5320 --yes
```

### Lokasi binary & PATH

`--bin DIR` menentukan folder instalasi. Defaultnya **auto**: folder pertama dari
`~/.local/bin`, `~/bin` yang **sudah ada di PATH shell login**; kalau dua-duanya tidak ada,
`~/.local/bin`.

Installer tidak mempercayai `$SHELL`. Dia **menanyakan shell login yang sebenarnya**
(lalu membunuhnya lagi dengan SIGKILL kalau rc-nya menggantung) untuk tahu PATH yang
benar-benar dipakai terminal baru, dan menulis PATH ke file rc yang shell itu baca:

| Shell login | File rc |
|---|---|
| zsh | `~/.zshenv` (dibaca semua zsh: login, interaktif, skrip) |
| bash | `~/.bashrc`, lalu escalate ke `~/.bash_profile` / `~/.bash_login` / `~/.profile` |
| fish | `~/.config/fish/config.fish` |

Setiap tulisan langsung **diverifikasi** dengan menanyakan ulang shell login. Ini penting di
host tanpa root: `/etc/passwd` masih menunjuk `/bin/bash` (karena `chsh` butuh root)
sementara `~/.bashrc` men-`exec` zsh user-local — di setup itu baris PATH di `~/.bashrc`
tidak pernah dieksekusi dan zsh tidak membaca `.bashrc` sama sekali. Login bash juga tidak
membaca `~/.bashrc`, jadi kalau verifikasi gagal installer naik ke kandidat berikutnya
alih-alih melaporkan sukses palsu.

Prioritas konfigurasi: **env variable per-call** > **config file** > *default bawaan*.
Installer juga mengecek dependency (`git`, `python3`, `lsof`, PostgreSQL client tools)
dan memvalidasi path repo/venv yang dimasukkan.

Perubahan `install.sh` sebaiknya diverifikasi dengan `./test-install.sh` (48 asersi:
deteksi shell login, penulisan PATH, integrasi fungsi shell, idempotensi, anti-hang).
Pakai `SKIP_LIVE=1 ./test-install.sh` untuk melewati asersi instalasi di host ini
(group 10).

### Integrasi shell: `dcw start` langsung pindah ke worktree

Setelah `dcw start <branch>` selesai, shell Anda otomatis `cd` ke worktree branch itu:

```sh
~/repo $ dcw start mybranch
✓ captain started: mybranch
  worktree: /home/you/repo/.worktrees/mybranch
~/repo/.worktrees/mybranch $        # ← sudah di dalam worktree
```

Ini **tidak bisa** dikerjakan binary-nya: proses anak tidak dapat mengubah working
directory shell pemanggilnya. Karena itu installer menulis **fungsi shell** tipis ke
file rc yang sama dengan blok PATH (zsh/bash):

- Hanya `start <branch>` yang menambah `cd`; semua subcommand lain diteruskan apa adanya.
- Target `cd` didapat dari subcommand baru `dcw path <branch>` — berguna manual juga:
  `cd "$(dcw path mybranch)"`.
- `cd` hanya terjadi kalau direktorinya benar-benar ada, dan exit code `dcw start`
  diteruskan apa adanya.
- `DCW_BIN=/path/lain/dcw` mengarahkan fungsi ke binary lain; `command dcw ...`
  melewati fungsi dan memanggil binary langsung.
- Shell login selain zsh/bash (mis. fish) belum dapat integrasi ini — installer hanya
  memberi peringatan, dan Anda `cd "$(dcw path <branch>)"` sendiri.
- Blok fungsi ditulis sekali (idempoten). Kalau Anda pindah `--bin`, installer akan
  memperingatkan bahwa blok lama masih menunjuk binary lain — hapus blok
  `# dcw shell integration (added by installer)` lalu jalankan installer lagi.

### Prasyarat

1. **Repo HMX** ada di `~/dev/hmx-002` (default; bisa di-override)
2. **Venv Python** di `~/dev/.venv` — berisi dependency `requirements.txt` + `hmx` CLI
3. **PostgreSQL** lokal dengan DB utama `hmxmain` (client tools di PATH:
   `psql`, `createdb`, `dropdb`, `pg_dump` — `brew install postgresql@16 libpq`)
4. Kredensial DB dibaca otomatis dari `<repo>/hmx/.env` (`DATABASE_*`)

Kalau lokasi Anda beda dari yang dipilih saat install, cukup set env sebelum memanggil (override sekali jalan):

```sh
DEV_CAPTAIN_REPO="$HOME/dev/hmx-002" DEV_CAPTAIN_VENV="$HOME/dev/.venv" dcw start <branch>
```

atau edit `~/.config/dcw/config`.

## Usage

```sh
dcw start <branch>    # buat worktree + clone DB + salin file privat + namespace Redis
                      # + jalankan server + worker Celery branch itu
                      # → shell otomatis cd ke worktree (lihat "Integrasi shell")
dcw status            # daftar captain aktif (port, pid, hidup/mati, celery)
dcw list              # daftar semua git worktree
dcw path <branch>     # cetak direktori branch (base → repo utama, lain → .worktrees/<branch>)
dcw logs <branch> [celery]   # tail log server, atau log worker branch itu
dcw stop <branch>     # stop server + worker (worktree + DB tetap)
dcw abort <branch>    # hapus worktree + stop server/worker + DROP DB branch (konfirmasi)

dcw celery start|restart|stop [branch]   # kelola worker (default: base branch)
dcw celery status     # semua worker: pid, hidup/mati, namespace Redis, log
```

Contoh:

```sh
dcw start wildantest
# ✓ captain started: wildantest
#   worktree: ~/dev/hmx-002/.worktrees/wildantest
#   port:     5201   preview: http://127.0.0.1:5201
#   db:       cloning hmxmain -> wildantest
#   celery:  worker up (pid 12345, ns wildan2-hmx-local-wildantest)

dcw start Inventory   # base branch → langsung di repo utama, port 5200
```

Base branch dideteksi dari branch yang aktif di repo (fallback `origin/HEAD`).

## Celery per branch

Setiap branch mendapat worker sendiri, terisolasi dari branch lain **tanpa** memakai
Redis DB index tambahan. Kuncinya `HMX_INSTANCE_NAMESPACE`: HMX menurunkan semua key
Redis dari `sha256(namespace)`, jadi dcw menulis namespace per-branch ke `.env` worktree:

```
worktree .env   HMX_INSTANCE_NAMESPACE = <namespace base>-<nama DB branch>
                contoh: wildan2-hmx-local-wildantest

hmx:<sha256(ns)>:celery:            queue + binding broker      (db6)
hmx:<sha256(ns)>:celery-result:     hasil task                   (db7)
hmx:<sha256(ns)>:cache              django cache + realtime      (db8)
```

Karena web dan worker branch membaca `.env` yang sama, keduanya otomatis memakai scope
yang sama. Branch lain tidak akan pernah mengambil task, hasil, cache, atau event
realtime milik branch ini — walaupun semuanya berbagi Redis DB 6/7/8 yang sama.

Queue default worker: `interactive,import,export,scheduled,notifications,csv,data_import,demo_pool,celery`
(semua queue yang dirutekan app, supaya task branch tidak menggantung tanpa konsumen).

Kesiapan worker:

- `dcw start` **tidak menunggu** worker siap (HMX memuat seluruh registry modul, ~15-30 detik).
  Outputnya `worker starting`, dan worker lanjut memuat di belakang.
- `dcw celery start` menunggu sampai siap, dan **gagal dengan exit 1** kalau worker tidak pernah siap.
- Status di `dcw status` / `dcw celery status`: `ready` (sudah siap), `starting` (masih memuat),
  `dead` (prosesnya mati/zombie), `-` (tidak ada worker).

Kegagalan worker tidak pernah menggagalkan `dcw start` — server tetap artefak utama.

**Beat tidak dijalankan per branch** — beberapa beat akan menembakkan jadwal yang sama
dua kali terhadap DB yang sama. Untuk base branch pakai `/opt/dev-tools/hmx-celery start beat`.

Override lewat env:

```sh
DCW_CELERY=0 dcw start <branch>            # jangan jalankan worker
DCW_CELERY_CONCURRENCY=1 dcw celery restart <branch>
DCW_CELERY_QUEUES=interactive dcw celery start <branch>
```

Catatan: worker base branch yang dijalankan dcw tidak berhubungan dengan
`/opt/dev-tools/hmx-celery`. Menjalankan keduanya hanya berarti ada dua worker pada
namespace base yang sama.

## State & kebersihan

| Path | Isi |
|---|---|
| `~/.dev-captain/ports.json` | registry port ↔ branch ↔ pid |
| `~/.dev-captain/logs/` | log server + `<branch>-celery.log` per branch |
| `~/.dev-captain/celery/<branch>.pid` | pid + nama branch worker (2 baris) |
| `<repo>/.worktrees/<branch>` | worktree (di-gitignore repo) |
| Postgres | DB per-branch (nama = nama branch, lowercase) |
| Redis | key per-branch via prefix `hmx:<sha256(namespace)>:` (DB index dibagi) |

File privat di worktree otomatis ditambahkan ke git exclude worktree — tidak akan ter-commit.

Saat `abort`, binding queue branch di Redis dibiarkan (jumlahnya kecil dan kedaluwarsa
sendiri). Verifikasi end-to-end: `./test-celery.sh`.

## Catatan

- `dcw abort` menghapus DB branch secara permanen (ada konfirmasi).
- DB `hmxmain` (utama) tidak pernah disentuh oleh `start`/`abort` worktree.
- Dirancang untuk macOS (`lsof`, `mkdir` lock; tanpa `fuser`/`ss`/`flock`).
