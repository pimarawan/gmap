pkgname=gmap
pkgver=0.5.0
pkgrel=1
pkgdesc="Find the risky parts of a git repository: hotspots, hidden coupling and the truck factor"
arch=('x86_64' 'aarch64')
url="https://github.com/pimarawan/gmap"
license=('MIT')
depends=()
makedepends=('rust' 'cargo')
source=("$pkgname-$pkgver.tar.gz::https://github.com/pimarawan/gmap/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
  cd "$pkgname-$pkgver"
  cargo build --release --locked
}

check() {
  cd "$pkgname-$pkgver"
  cargo test --release --locked
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 "target/release/gmap" "$pkgdir/usr/bin/gmap"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
