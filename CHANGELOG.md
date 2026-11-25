# Changelog

Semua perubahan dcw yang masuk ke rilis. Format mengikuti [Keep a Changelog], versi
mengikuti [Semantic Versioning]. Satu bagian per tag rilis — judulnya dipakai sebagai
catatan rilis di GitHub dan ditampilkan oleh `dcw update` setelah binary diganti.

Rilis pertama yang tersedia untuk umum adalah **0.1.0**; tidak ada tag 0.2.1 dan 0.2.3.

## 0.2.13 — 2026-11-25

### Added

- Subcommand baru `dcw ram`: pemakaian RAM user saat ini, dirinci per worktree dcw
  (kolom server dan worker — anak autoreload dan child Celery ikut terhitung karena
  dijumlahkan per process group, mengandalkan PGID == PID dari proses dcw), lalu proses
  lain yang bukan milik dcw dikelompokkan per nama command, ditutup total dcw dan total
  user. Penjumlahan RSS menghitung ganda memori shared antar proses, jadi angkanya
  indikatif, bukan byte-exact. Proses di luar dcw dilabeli nama command; untuk runtime
  generik (`node`, `python`) label-nya nama skrip pertama yang dijalankan.

### Fixed

- Semua subcommand yang di-pipe ke program yang menutup pipe lebih dulu (mis.
  `dcw ram | head`) kini berakhir senyap seperti tool Unix pada umumnya; sebelumnya
  dcw panic `failed printing to stdout: Broken pipe`.

## 0.2.12 — 2026-11-24

### Added

- Subcommand baru `dcw config`: menampilkan config efektif lalu menanyakan apakah worker
  Celery dijalankan secara default (`dcw new`/`dcw start`). Jawaban diedit **in-place** di
  `~/.config/dcw/config` — hanya baris `DCW_CELERY` yang berubah, key lain yang kamu tulis
  manual di file tidak tergeser (tidak seperti rewrite penuh oleh `dcw init`).
- `dcw init` kini menulis `DCW_CELERY` ke file config — nilai yang sudah ada dipertahankan
  (default `1`) — dan menampilkannya sebagai baris `celery:` di blok "Current configuration".
  Opsi mematikan worker bawaan (`DCW_CELERY=0`) jadi terlihat dan bisa diedit langsung di
  `~/.config/dcw/config`; sebelumnya key ini harus ditulis manual dan hilang setiap kali
  `dcw init` dijalankan ulang.

### Changed

- Saat `DCW_CELERY=0`, `dcw new` tidak lagi mencetak baris "celery skipped" di tengah
  progress block (baris rutin memang diringkas saat block aktif); `dcw start`, `dcw restart`,
  dan `dcw celery start` tetap mencetak alasan tersebut.

## 0.2.11 — 2026-11-23

### Fixed

- `dcw new` kini memakai kembali port milik worktree yang sudah tidak ada. Sebelumnya
  registry hanya dibersihkan oleh `dcw abort`, jadi worktree yang dihapus dengan cara lain
  (`git worktree remove`, `rm -rf`) meninggalkan port-nya terkunci selamanya dan setiap
  `dcw new` berikutnya mulai satu nomor lebih tinggi — tanpa terlihat di `dcw list`.
  Entry yang dibuang hanya yang worktree-nya tidak ada di `git worktree list` **dan**
  tidak punya proses hidup; base branch dan worktree yang di-`stop` tetap memegang port-nya.

## 0.2.10 — 2026-11-21

### Added

- `dcw update` kini menampilkan changelog setelah binary diganti, mencakup semua rilis
  dari versi yang sedang berjalan sampai versi yang baru dipasang — jadi lompat beberapa
  versi tetap terlihat semuanya. Catatan diambil dari binary yang baru diunduh, sehingga
  rilis lama pun bisa menceritakan rilis baru.
- `dcw changelog [versi]` menampilkan catatan rilis langsung, tanpa meng-update.

### Notes

- Mulai rilis ini changelog ikut tertanam di binary. Rilis **0.2.9 ke bawah** tidak
  memilikinya, jadi `dcw update` ke 0.2.10 akan menampilkan catatan 0.2.10 saja; update
  dari 0.2.10 ke atas menampilkan seluruh rentang versi yang dilewati.

## 0.2.9 — 2026-11-21

### Added

- `dcw new` menerima beberapa nama sekaligus: `dcw new a b c` atau `dcw new a,b,c`.
  Maksimal 3 worktree dibangun serentak, satu baris progress per worktree. `git fetch`
  dan penentuan titik awal hanya sekali untuk seluruh batch.
- Perintah `new` tanpa nama tidak lagi bingung: dipakai sebagai error pemakaian.

### Fixed

- Progress checkout tidak lagi menggantung di 98%: sampler berhenti saat checkout
  selesai, bukan saat hitungan mencapai 100% (git tidak selalu menulis persis
  sebanyak file yang dilaporkan `ls-tree`).

## 0.2.8 — 2026-11-21

### Changed

- `dcw abort` beberapa worktree berjalan paralel, satu baris progress per worktree
  (sebelumnya satu per satu dengan output saling menyela).

## 0.2.7 — 2026-11-21

### Added

- Progress bar 0-100% yang nyata untuk langkah checkout di `dcw new` — angkanya dari
  file yang benar-benar sudah ditulis git, dibagi jumlah file yang dilaporkan
  `git ls-tree`.

## 0.2.6 — 2026-11-20

### Added

- Spinner untuk langkah-langkah senyap di `dcw new` (fetch, clone DB), sehingga tidak
  ada lagi jeda tanpa keterangan.

### Fixed

- Nama file sementara registry dibuat unik per pemanggilan, sehingga dua penyimpanan
  yang berjalan bersamaan tidak saling menimpa.

## 0.2.5 — 2026-11-20

### Added

- Aset rilis macOS (arm64 + Intel) dibangun otomatis oleh GitHub Actions.
- `dcw update` menghapus atribut `com.apple.quarantine` setelah mengunduh, sehingga
  binary hasil update bisa langsung dijalankan di macOS.

### Changed

- `dcw new` dan `dcw abort` lebih cepat.

## 0.2.4 — 2026-11-20

### Added

- Menu `dcw start`/`dcw stop` menampilkan repo utama sebagai baris `(repo)`.

### Fixed

- Menu `dcw restart` memilih target dengan benar.
- Registry dikunci dengan `flock` (lock dilepas kernel saat proses mati, jadi tidak
  ada lock tersangkut) dan `ports.json` ditulis atomik.

## 0.2.2 — 2026-11-16

### Changed

- **Identitas target adalah nama worktree, bukan nama branch.** `start`, `stop`,
  `restart`, `abort`, `logs` menerima nama direktori worktree; nama branch tetap
  diterima sebagai alias. Ini karena satu worktree bisa di-`git switch` kapan saja
  tanpa kehilangan port, log, dan worker-nya.
- `dcw new` melakukan `cd` ke worktree yang baru dibuat (butuh fungsi shell `dcw()`).

### Fixed

- Nama worktree diresolusi dengan benar lewat path repo yang bersimlink (macOS).

## 0.2.0 — 2026-11-13

### Added

- `dcw restart`: stop lalu start server + worker, tanpa menyentuh worktree, DB, dan port.

## 0.1.2 — 2026-11-13

### Fixed

- `dcw list` menampilkan captain yang sedang jalan walaupun branch yang checked-out di
  worktree-nya berbeda dari kunci di registry.
- Bug argumen `lsof`: port ikut terbelah dari `-tiTCP:<port>`, sehingga `stop` dan
  `abort` meninggalkan server yatim di port-nya.

### Changed

- README dirapikan (install/usage/config lebih ringkas).

## 0.1.1 — 2026-11-09

### Added

- Dukungan macOS: `pid_alive` lintas platform, plus workflow rilis GitHub Actions.

## 0.1.0 — 2026-11-09

Rilis publik pertama.

### Added

- Port Rust dari script bash `dcw` (project `wtc`), menggantikan implementasi bash.
- Installer `curl | bash` dan `dcw update` yang mengambil binary prebuilt dari GitHub
  Releases.
- Lingkungan terisolasi penuh per feature branch HMX: worktree, port, clone DB Postgres,
  namespace Redis, server Django, dan worker Celery sendiri.
- `dcw new`, `start`, `stop`, `list`, `jump`, `abort`, `logs`, `celery`, `init`,
  `uninstall`.
- Menu panah untuk start/stop/abort/logs saat dijalankan tanpa nama.
- Server dan worker di-spawn dengan `setsid`, sehingga terminal yang mati tidak
  membunuh mereka.
- URL preview otomatis lewat domain wildcard DevSpace.

[Keep a Changelog]: https://keepachangelog.com/id/1.1.0/
[Semantic Versioning]: https://semver.org/lang/id/