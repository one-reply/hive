# Іконки Hive

Модульна H: три окремі фігури, одна геометрія для всіх розмірів.
Світла версія — чорний знак на білому, темна — білий на чорному.

| Файл | Призначення |
| --- | --- |
| `Hive.icon/` | Редаговане джерело Icon Composer зі світлим і темним appearance; SVG у `Assets/mark.svg` |
| `hive-light.png`, `hive-dark.png` | Прозоре полотно 1024×1024, macOS-відступ 100 px навколо плитки 824×824 |
| `Hive-light.iconset/`, `Hive-dark.iconset/` | По 10 PNG: 16, 32, 128, 256, 512 pt у масштабах 1× і 2×; фізично до 1024 px |
| `Hive-light.icns`, `Hive-dark.icns` | Повні статичні набори для macOS |
| `source/generated-mark.png` | Генеративний ескіз до нормалізації геометрії у SVG; не ресурс застосунку |

У GUI `Icon::data` використовує той самий SVG як маску й бере колір із поточної теми.
Логотип є у title bar і на welcome-екрані.

## Відтворення

З кореня репозиторію, на macOS з Xcode 26+ (Icon Composer), Swift та Python 3:

```sh
bash scripts/generate-icons.sh
python3 scripts/check-icons.py
bash scripts/bundle-macos.sh          # target/debug/bundle/Hive.app
bash scripts/bundle-macos.sh release  # target/release/bundle/Hive.app
open target/debug/bundle/Hive.app
```

`generate-icons.sh` експортує обидва appearance штатним `ictool`, додає macOS-відступ
через CoreGraphics, масштабує PNG через `sips` і збирає `.icns` через `iconutil`.
Перевірка контролює PNG-розміри/alpha та наявність малих і Retina-зображень у `.icns`.

Пакування компілює `.icon` через `actool` у `Assets.car`, перевіряє обидва appearance,
записує версію Cargo й мінімальну macOS із Mach-O у `Info.plist`, додає нативний `.icns`
і ставить локальний ad-hoc підпис. Іконка доступна Finder, Dock і перемикачу застосунків.
Це локальна збірка для поточної архітектури; Developer ID та notarization сюди не входять.

macOS 26+ обирає appearance нативної іконки за системним стилем іконок. Налаштування
теми всередині Hive змінює його інтерфейс. На старішій macOS використовується статичний
світлий fallback. Звичайний `cargo run` запускає бінарник без `.app`-метаданих;
для перевірки Finder/Dock запускайте зібраний `Hive.app`.

## Джерела

- [Apple: Icon Composer](https://developer.apple.com/documentation/xcode/creating-your-app-icon-using-icon-composer)
- [Apple: склад iconset](https://developer.apple.com/library/archive/documentation/Xcode/Reference/xcode_ref-Asset_Catalog_Format/IconSetType.html)
- Генеративний ескіз: вбудований `imagegen`, [збережений prompt](source/prompt.md).
