# Red Text Editor

Editor teks desktop ringan yang ditulis dengan Rust dan [egui](https://github.com/emilk/egui) (eframe). Tampilannya meniru VS Code (tema Dark Modern / Light Modern) dan antarmukanya berbahasa Indonesia.

## Fitur

- **Banyak tab** bergaya VS Code: badge bahasa di tiap tab, titik `●` untuk file yang belum disimpan, tombol `×` saat di-hover, tombol `+` untuk tab baru.
- **Syntax highlighting** untuk Rust, C, C++, PHP, JavaScript, TypeScript dan HTML, dikenali dari ekstensi file. Bahasa bisa diganti manual lewat menu **Tampilan → Bahasa**. File di atas 1 MB ditampilkan tanpa highlighting.
- **Nomor baris** yang bisa ditampilkan atau disembunyikan (**Tampilan → Nomor Baris**). Nomor baris tempat kursor berada tampil lebih terang.
- **Bungkus baris** (word wrap) yang bisa dinyalakan atau dimatikan (**Tampilan → Bungkus Baris**). Saat mati, baris panjang bisa digeser ke samping.
- **Explorer dan Activity Bar** seperti VS Code: buka folder, jelajahi isinya sebagai tree, klik file untuk membukanya di tab. Ada tombol **Refresh** dan **Collapse All** di header folder.
- **Konfirmasi perubahan belum disimpan** saat menutup tab atau keluar dari aplikasi.
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

Hasil build release ada di `target/release/rred_text_editor`.

## Install

### Dengan cargo (disarankan)

```bash
cargo install --path .
```

Program dipasang ke `~/.cargo/bin/rred_text_editor`. Pastikan `~/.cargo/bin` ada di `PATH` (rustup biasanya sudah mengaturnya), lalu jalankan:

```bash
rred_text_editor
```

Untuk menghapus:

```bash
cargo uninstall rred_text_editor
```

### Manual, untuk semua pengguna (Linux)

```bash
cargo build --release
sudo install -Dm755 target/release/rred_text_editor /usr/local/bin/rred_text_editor
```

Untuk menghapus: `sudo rm /usr/local/bin/rred_text_editor`.

### Shortcut di menu aplikasi (Linux, opsional)

Supaya muncul di menu aplikasi desktop, buat file `~/.local/share/applications/red-text-editor.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=Red Text Editor
Comment=Editor teks ringan bergaya VS Code
Exec=rred_text_editor
Terminal=false
Categories=Utility;TextEditor;Development;
```

Kalau dipasang di luar `PATH`, isi `Exec=` dengan path lengkapnya, misalnya `Exec=/home/nama/.cargo/bin/rred_text_editor`.

## Cara pakai

| Menu | Isi |
|---|---|
| **File** | Tab Baru, Tutup Tab, Buka File…, Buka Folder…, Simpan, Simpan Sebagai…, Bahasa, Keluar |
| **Tampilan** | Bungkus Baris, Nomor Baris, Bahasa |
| **Help** | About |

- Klik ikon **Explorer** di Activity Bar (kolom paling kiri) untuk membuka folder. Setelah folder terbuka, ikon yang sama menyembunyikan atau menampilkan sidebar.
- Klik folder di tree untuk membuka atau menutupnya, klik file untuk membukanya. File yang sudah terbuka tidak dibuka dua kali; tab-nya yang difokuskan.
- Isi folder dibaca sekali lalu disimpan. Klik **Refresh** di header folder kalau ada file baru dari luar editor.

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
| `src/highlighter.rs` | Syntax highlighting (syntect) |
| `src/file_browser.rs` | Explorer: tree folder dan sidebar |
| `src/activity_bar.rs` | Activity Bar |
| `src/modals.rs` | Dialog About dan konfirmasi simpan |

Desain dan rencana tiap fitur ada di `docs/superpowers/specs/` dan `docs/superpowers/plans/`.
