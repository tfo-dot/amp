set shell := ["bash", "-c"]

pkg_name        := `grep -m 1 '^name' Cargo.toml | cut -d '"' -f 2`
pkg_description := `grep -m 1 '^description' Cargo.toml | cut -d '"' -f 2`
binary_path     := "/usr/bin/" + pkg_name

gen-pkg: desktop
    @echo "Generating AUR package for {{pkg_name}}..."
    cargo aur
    @echo "Package generated in target/cargo-aur/"

desktop:
    @echo "Generating {{pkg_name}}.desktop..."
    @echo "[Desktop Entry]" > {{pkg_name}}.desktop
    @echo "Type=Application" >> {{pkg_name}}.desktop
    @echo "Name={{pkg_name}}" >> {{pkg_name}}.desktop
    @echo "Comment={{pkg_description}}" >> {{pkg_name}}.desktop
    @echo "Exec={{binary_path}}" >> {{pkg_name}}.desktop
    @echo "Icon={{pkg_name}}" >> {{pkg_name}}.desktop
    @echo "Terminal=false" >> {{pkg_name}}.desktop
    @echo "Categories=Utility;Development;" >> {{pkg_name}}.desktop

install: gen-pkg
    cd target/cargo-aur && makepkg -si --noconfirm

clean:
    rm -f *.desktop
    rm -rf target/cargo-aur