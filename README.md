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

### Helper venv: `venvhmx` dan `venvhere`

Installer juga memasang dua fungsi venv dari `shell/venv.sh` di repo (disalin ke
`~/.config/dcw/shell/venv.sh`, lalu di-`source` dari file rc yang sama):

| Fungsi | Efek |
|---|---|
| `venvhmx` | activate venv project **dan** `cd` ke folder project |
| `venvhere` | activate venv **tanpa** pindah direktori — tetap di tempat Anda sekarang |

`venvhere` dipasangkan dengan `dcw start`: setelah `dcw start <branch>` Anda sudah
berada di worktree, lalu `venvhere` mengaktifkan venv tanpa memindahkan Anda lagi:

```sh
~/repo $ dcw start mybranch
✓ captain started: mybranch
~/repo/.worktrees/mybranch $ venvhere
venvhere: aktif -> Python 3.14.4 @ /home/you/repo/.worktrees/mybranch
```

Urutan pencarian venv `venvhere`: argumen (`venvhere /path/venv`) → `.venv` di
direktori sekarang → `.venv` di root git/worktree → venv dari config dcw
(`DEV_CAPTAIN_VENV`, default `~/.venv`). Venv yang sedang aktif dilepas dulu
supaya PATH tidak menumpuk; path yang salah → pesan error + exit 1.

`venvhmx` dan `venvhere` membaca `~/.config/dcw/config`, jadi keduanya mengikuti
`DEV_CAPTAIN_REPO` / `DEV_CAPTAIN_VENV` — pindah project cukup ubah config, tidak
ada path yang di-hardcode di dotfile.

Karena keduanya berasal dari repo, Anda tidak perlu menaruh definisinya manual di
`~/.zshrc` lagi. Kalau masih ada blok manual lama, sebaiknya dihapus: `~/.zshenv`
dibaca sebelum `~/.zshrc`, jadi definisi manual yang lama akan **menang** dan
versi repo tidak terpakai.

Selain itu, installer juga menambahkan baris `source` ke file rc (marker
`# dcw venv helpers (added by installer)`), jadi cukup `install.sh` sekali.

Memperbarui: jalankan `install.sh` lagi. File `~/.config/dcw/shell/venv.sh`
ditimpa dengan versi repo, jadi fungsi ikut ter-update tanpa mengubah rc file.
`uninstall.sh` menghapus binary, `~/.config/dcw`, dan ketiga blok rc (dengan
backup `<rc>.dcw-uninstall.bak`).

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
dcw status            # daftar captain aktif (port, pid, hidup/mati, celery, URL)
dcw list              # daftar semua git worktree
dcw path <branch>     # cetak direktori branch (base → repo utama, lain → .worktrees/<branch>)
dcw logs <branch> [celery]   # tail log server, atau log worker branch itu
dcw stop [branch]     # stop server + worker (worktree + DB tetap)
                      # tanpa <branch> → menu pilihan captain yang jalan
dcw abort [branch]    # hapus worktree + stop server/worker + DROP DB branch (konfirmasi)
                      # tanpa <branch> → menu pilihan worktree milik dcw

dcw celery start|restart|stop [branch]   # kelola worker (default: base branch)
dcw celery status     # semua worker: pid, hidup/mati, namespace Redis, log
```

### stop / abort tanpa argumen (menu pilihan)

`dcw stop` dan `dcw abort` tanpa `<branch>` menampilkan menu bernomor supaya nama
branch tidak perlu diingat atau diketik:

```
$ dcw stop
#    BRANCH                                 PORT    STATUS
1    P-DB-11612                             4201    up
2    inven-standard-rules                   4203    up
3    Inventory                              4200    down
Select (number(s), branch name, q = cancel): 1
```

- Kandidat `stop` = captain di registry port; yang hidup ditampilkan lebih dulu.
- Kandidat `abort` = worktree yang dikelola dcw saja (anak `$WT_DIR`); repo utama dan
  worktree di luar `$WT_DIR` tidak pernah ditawarkan.
- Pilihan: satu nomor, beberapa nomor (`1 3` atau `1,3`), atau nama branch langsung.
  Duplikat dibuang. `q` membatalkan. Kalau kandidatnya cuma satu, Enter memilihnya.
  Salah input diberi kesempatan 3 kali, lalu batal.
- `abort` multi-target memakai satu konfirmasi gabungan (`Proceed? [y/N]`).
- Hanya interaktif: kalau stdin bukan TTY (dipakai script/cron), perintah langsung
  keluar dengan usage + exit 2, bukan menggantung menunggu input.
- `dcw stop <branch>` dan `dcw abort <branch>` dengan argumen tidak berubah perilakunya.

Contoh:

```sh
dcw start wildantest
# ✓ captain started: wildantest
#   worktree: ~/dev/hmx-002/.worktrees/wildantest
#   port:     5201   preview: https://<user>-5201.devx.hashmicro.co
#   db:       cloning hmxmain -> wildantest
#   celery:  worker up (pid 12345, ns wildan2-hmx-local-wildantest)

dcw start <base-branch>   # base branch → langsung di repo utama, port 5200
```

Base branch dideteksi dari `origin/HEAD` (branch default yang dideklarasikan repo,
sama seperti `dev-captain`); fallback ke branch yang sedang aktif di repo, lalu `main`.
Untuk memaksa: `DEV_CAPTAIN_BASE=<branch> dcw start <branch>`.

### Titik awal branch baru

Branch baru dibuat dari **`origin/<base>`** sesudah `git fetch origin`, jadi mulai dari
commit terbaru di remote — bukan dari ref lokal `<base>`. Ref lokal `<base>` hanya ikut
bergerak kalau repo utama di-`pull`/`switch`, dan karena repo utama biasanya sedang
men-checkout `<base>` itu, `git fetch` tidak pernah memperbaruinya. Urutan start-point:

| prioritas | ref | kapan dipakai |
|---|---|---|
| 1 | `refs/heads/<branch>` | branch sudah ada lokal — dipakai apa adanya (tidak di-reset) |
| 2 | `refs/remotes/origin/<base>` | normal: branch baru dari commit terbaru remote |
| 3 | `refs/heads/<base>` | remote base belum ada (mis. repo tanpa `origin`) |

Branch baru dibuat dengan `--no-track`, jadi `git push` tidak mengarah ke `origin/<base>`.
Kalau kamu memang ingin tracking seperti `codex-wt`, set sendiri:
`git branch -u origin/<base>` (hanya kalau branch itu memang salinan base).

Server base (`dcw start <base>`, port `PORT_BASE`) tetap menyajikan apa yang sedang
di-checkout di repo utama — dcw tidak menyentuh working tree repo utama. Supaya ikut
latest: `git -C <repo> pull --ff-only`.

## Preview URL (nginx wildcard DevSpace)

Kolom URL di `dcw status` dan baris `preview:` di `dcw start` berisi alamat yang bisa
langsung dibuka di browser. Kalau host punya reverse proxy wildcard DevSpace
(`/etc/devspace-nginx-routes.env`), dcw memakai skema yang sama dengan
`/opt/dev-tools/dev-captain`:

| Branch | URL | Diteruskan nginx ke |
|---|---|---|
| base (`$BASE`) | `https://<user>.<domain>` | `127.0.0.1:$PORT_BASE` |
| worktree | `https://<user>-<port>.<domain>` | `127.0.0.1:<port>` |

Domain dideteksi otomatis: env `DEVSPACE_DOMAIN`, lalu `/etc/devspace-nginx-routes.env`.
Tanpa keduanya (mis. Mac lokal) URL-nya tetap `http://127.0.0.1:<port>` seperti sebelumnya.

Override:

```sh
DCW_DOMAIN=example.test DCW_USER=bob dcw status   # domain/user lain
DCW_DOMAIN= dcw status                            # paksa http://127.0.0.1:<port>
```

`DCW_DOMAIN` boleh juga ditaruh di `~/.config/dcw/config`; env per-call tetap menang.

URL itu hanya hidup kalau nginx di host benar-benar memetakan host tersebut ke port itu:
`<user>-<port>` hanya untuk port di range yang diberikan ke user (`/etc/apollo-devhost/user-ports.map`),
dan `<user>.<domain>` selalu menunjuk ke **base port** user itu — jadi URL base branch baru
benar kalau `PORT_BASE` di config dcw sama dengan base port yang diberikan host.

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
