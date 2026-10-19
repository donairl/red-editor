[English](README.md) | **Bahasa Indonesia**

---

# Red Text Editor

Editor teks desktop ringan yang ditulis dengan Rust dan [egui](https://github.com/emilk/egui) (eframe). Tampilannya meniru VS Code (tema Dark Modern / Light Modern) dan antarmukanya berbahasa Indonesia.

## Fitur

- **Banyak tab** bergaya VS Code: badge bahasa di tiap tab, titik `●` untuk file yang belum disimpan, tombol `×` saat di-hover, tombol `+` untuk tab baru.
- **Syntax highlighting** untuk Rust, C, C++, PHP, JavaScript, TypeScript, HTML, CSS (juga `.scss` dan `.less`), JSON, Markdown, Go, Python, SQL, serta template Blade (`.blade.php`), Twig (`.twig`), dan Smarty (`.tpl`), dikenali dari ekstensi file. Bahasa bisa diganti manual lewat menu **Tampilan → Bahasa**. File di atas 1 MB ditampilkan tanpa highlighting.
- **SQL di dalam string**: string di Rust, C, C++, PHP, JavaScript, TypeScript, Go, atau Python yang berisi perintah SQL (`"SELECT id FROM users"`, `` `\n  SELECT …` ``) diwarnai kata kunci SQL-nya, termasuk string multi-baris. Kata kunci huruf besar sudah cukup; huruf kecil butuh klausa kedua (`select … from`), jadi teks UI seperti "Select a file" tetap berwarna string biasa.
- **Nomor baris** yang bisa ditampilkan atau disembunyikan (**Tampilan → Nomor Baris**). Nomor baris tempat kursor berada tampil lebih terang.
- **Bungkus baris** (word wrap) yang bisa dinyalakan atau dimatikan (**Tampilan → Bungkus Baris**). Saat mati, baris panjang bisa digeser ke samping.
- **Explorer dan Activity Bar** seperti VS Code: buka folder, jelajahi isinya sebagai tree, klik file untuk membukanya di tab. Ada tombol **Refresh** dan **Collapse All** di header folder.
- **Cari dan ganti** (**Edit → Cari**, **Edit → Ganti**): bar di atas editor dengan hasil yang disorot, opsi huruf besar/kecil, dan Ganti Semua. Ada juga **Ke Baris…** untuk melompat ke nomor baris.
- **Cari di File** (**Edit → Cari di File**, `Ctrl+Shift+F`): mencari teks di seluruh folder yang sedang dibuka, di thread terpisah. Hasil tampil di panel bawah; klik hasil untuk membuka file di baris itu. File dan folder tersembunyi, `target`, `node_modules`, file di atas 2 MB, dan file non-UTF-8 dilewati; hasil dibatasi 2000.
- **Menu klik kanan pada tab**: Tutup Tab, Tutup Lainnya, Tutup Tab di Kanan, dan Tutup yang Tersimpan. Tab yang belum disimpan tidak ikut tertutup.
- **Konfirmasi perubahan belum disimpan** saat menutup tab atau keluar dari aplikasi.
- **Multi bahasa antarmuka** (Indonesia, English, Español, Deutsch, 中文, dan bahasa lain lewat file JSON). Pilih di **Tampilan → Bahasa Antarmuka**; pilihan diingat saat aplikasi dibuka lagi. Lihat [Menambah bahasa antarmuka](#menambah-bahasa-antarmuka).
- **Zen mode** (`F11`): mode layar penuh tanpa gangguan, sembunyikan semua panel selain editor; tekan `Esc` untuk keluar.
- **Prettify** (**Edit → Rapikan**, `Ctrl+Shift+I`): merapikan JSON, HTML, dan CSS langsung di editor, serta Rust, Go, C/C++, Python, JS/TS lewat formatter yang terpasang (`rustfmt`, `gofmt`, `clang-format`, `ruff`/`black`, `prettier`).
- **Pratinjau Markdown** (`Ctrl+Shift+V`): hasil render file `.md` dalam tampilan terbelah langsung di samping editor.
- Tema terang/gelap mengikuti pengaturan sistem.

## Kebutuhan

- **Rust 1.95 atau lebih baru** (edition 2024). Cara termudah memasang Rust: [rustup](https://rustup.rs).

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- **Linux:** library sistem yang dibutuhkan eframe. Di Debian/Ubuntu:

  ```bash
  sudo apt-get install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
  ```

  Dialog buka/simpan file memakai **xdg-desktop-portal**. Pastikan portal dan salah satu backend-nya terpasang (misalnya `xdg-desktop-portal-gtk`, `-kde` atau `-gnome`); di kebanyakan desktop modern ini sudah ada.

Build sudah diuji di Arch Linux dengan Rust 1.99. eframe dan rfd juga mendukung Windows dan macOS, tetapi build di sana belum diuji.

## Compile

```bash
git clone <url-repo> red-text-editor
cd red-text-editor

# Build debug dan langsung jalankan
cargo run

# Build release (lebih cepat dan lebih kecil)
cargo build --release
```

Hasil build release ada di `target/release/red_text_editor`.

## Install

### Dengan cargo (disarankan)

```bash
cargo install --path .
```

Program dipasang ke `~/.cargo/bin/red_text_editor`. Pastikan `~/.cargo/bin` ada di `PATH` (rustup biasanya sudah mengaturnya), lalu jalankan:

```bash
red_text_editor
```

Untuk menghapus:

```bash
cargo uninstall red_text_editor
```

### Manual, untuk semua pengguna (Linux)

```bash
cargo build --release
sudo install -Dm755 target/release/red_text_editor /usr/local/bin/red_text_editor
```

Untuk menghapus: `sudo rm /usr/local/bin/red_text_editor`.

### Shortcut di menu aplikasi (Linux, opsional)

Supaya muncul di menu aplikasi desktop, buat file `~/.local/share/applications/red-text-editor.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=Red Text Editor
Comment=Editor teks ringan bergaya VS Code
Exec=red_text_editor
Terminal=false
Categories=Utility;TextEditor;Development;
```

Kalau dipasang di luar `PATH`, isi `Exec=` dengan path lengkapnya, misalnya `Exec=/home/nama/.cargo/bin/red_text_editor`.

## Cara pakai

| Menu | Isi |
|---|---|
| **File** | Tab Baru, Tutup Tab, Buka File…, Buka Folder…, Simpan, Simpan Sebagai…, Bahasa, Keluar |
| **Edit** | Urungkan, Ulangi, Potong, Salin, Tempel, Cari, Ganti, Cari di File, Ke Baris… |
| **Tampilan** | Bungkus Baris, Nomor Baris, Zen Mode, Pratinjau Markdown, Bahasa, Bahasa Antarmuka |
| **Help** | About |

### Shortcut keyboard

Di macOS, `Ctrl` menjadi `Cmd`.

| Shortcut | Aksi |
|---|---|
| `Ctrl+N` | Tab baru |
| `Ctrl+W` | Tutup tab |
| `Ctrl+O` | Buka file |
| `Ctrl+Shift+O` | Buka folder |
| `Ctrl+S` | Simpan |
| `Ctrl+Shift+S` | Simpan sebagai |
| `Ctrl+Q` | Keluar |
| `Ctrl+B` | Tampilkan/sembunyikan sidebar |
| `F11` | Zen mode (layar penuh, hanya editor); `Esc` untuk keluar |
| `Ctrl+Shift+V` | Tampilkan/sembunyikan pratinjau Markdown (file `.md`) |
| `Ctrl+Shift+I` | Rapikan dokumen |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Tab berikutnya / sebelumnya |
| `Ctrl+Z` | Urungkan |
| `Ctrl+Y` atau `Ctrl+Shift+Z` | Ulangi |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | Potong / Salin / Tempel |
| `Ctrl+F` | Cari |
| `Ctrl+H` | Ganti |
| `F3` / `Shift+F3` | Hasil cari berikutnya / sebelumnya |
| `Ctrl+Shift+F` | Cari di File |
| `Ctrl+G` | Ke baris |
| `Esc` | Tutup bar cari, bar ke baris, atau panel Cari di File |

Shortcut juga tampil di sebelah item menu yang bersangkutan.

### Explorer

- Klik ikon **Explorer** di Activity Bar (kolom paling kiri) untuk membuka folder. Setelah folder terbuka, ikon yang sama menyembunyikan atau menampilkan sidebar.
- Klik folder di tree untuk membuka atau menutupnya, klik file untuk membukanya. File yang sudah terbuka tidak dibuka dua kali; tab-nya yang difokuskan.
- Isi folder dibaca sekali lalu disimpan. Klik **Refresh** di header folder kalau ada file baru dari luar editor.

## Menambah bahasa antarmuka

Setiap bahasa adalah satu file JSON di `assets/lang/`, dinamai dengan kode bahasanya (`id.json`, `en.json`, `ja.json`, …). Isinya objek datar berisi kunci dan teks; `"name"` adalah nama bahasa yang tampil di menu, dan token seperti `{error}` atau `{name}` diisi otomatis oleh aplikasi.

```json
{
  "name": "日本語",
  "menu.file": "ファイル",
  "menu.view": "表示",
  "msg.open_failed": "ファイルを開けませんでした: {error}"
}
```

Langkah:

1. Salin `assets/lang/en.json` menjadi `assets/lang/<kode>.json`, lalu terjemahkan nilainya (jangan ubah kuncinya).
2. Taruh file itu di folder `assets/lang/` di **samping file program** (misalnya `~/.cargo/bin/assets/lang/` untuk hasil `cargo install`, atau `/usr/local/bin/assets/lang/`), atau di `assets/lang/` pada **folder tempat aplikasi dijalankan**.
3. Buka ulang aplikasi; bahasa baru muncul di **Tampilan → Bahasa Antarmuka**.

Kunci yang tidak ada di file memakai teks Bahasa Indonesia. File dengan kode yang sama dengan bahasa bawaan (mis. `id.json`, `en.json`, `zh.json`) hanya menimpa kunci yang ada di dalamnya. Kalau file tidak valid, aplikasi tetap jalan dan menampilkan pesan error di status bar.

Bahasa Indonesia, English, Español, Deutsch, dan 中文 sudah tertanam di dalam program, jadi tetap tersedia walaupun folder `assets/` tidak ikut dipasang. Saat pertama kali dibuka, aplikasi memilih bahasa sistem (`LANG`) kalau tersedia, selain itu English. Teks Mandarin memakai font CJK sistem (mis. Noto Sans CJK) kalau terpasang.

## Pengembangan

```bash
cargo test -- --skip test_save_tab_restores_active_tab_index
cargo clippy --all-targets
```

Test `test_save_tab_restores_active_tab_index` di-skip karena membuka dialog Save sungguhan dan menahan `cargo test` sampai dialog itu ditutup.

Struktur kode:

| File | Isi |
|---|---|
| `src/main.rs` | Titik masuk, membuat jendela |
| `src/app.rs` | State aplikasi, menu, panel, editor |
| `src/tab_bar.rs` | Tab strip bergaya VS Code |
| `src/gutter.rs` | Gutter nomor baris |
| `src/find.rs` | Cari/ganti, bar cari dan bar Ke Baris |
| `src/search.rs` | Cari di File: pencarian folder dan panel hasil |
| `src/highlighter.rs` | Syntax highlighting (syntect) |
| `src/embedded_sql.rs` | Pewarnaan SQL di dalam string |
| `src/file_browser.rs` | Explorer: tree folder dan sidebar |
| `src/activity_bar.rs` | Activity Bar |
| `src/modals.rs` | Dialog About dan konfirmasi simpan |
| `src/i18n.rs` | Terjemahan antarmuka dari `assets/lang/*.json` |
| `assets/lang/` | File bahasa antarmuka |

Desain dan rencana tiap fitur ada di `docs/superpowers/specs/` dan `docs/superpowers/plans/`.
