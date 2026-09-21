# Сборка FocusMute и настройка Scarlett Solo 4th Gen

Дата: 21 сентября 2026. Инструкция относится к исходникам в этом репозитории
и к FocusMute 0.11.0.

## Что реализовано для двух моделей

Приложение определяет модель из имени, которое сообщает драйвер Focusrite.
Для `Scarlett 2i2 4th Gen` остаётся прежняя ветка: номера входов меняются
через старые поля и события устройства. Для `Scarlett Solo 4th Gen` выбрана
другая ветка протокола:

* одиночный LED использует `directLEDColour=80`, `directLEDIndex=84` и
  `DATA_NOTIFY(8)`;
* `indicator.mode = "auto"` во время mute меняет выбранные номера каналов:
  1 — индекс 4, 2 — индекс 12. Карта bulk-массива LED Solo не используется, поэтому
  приложение не пишет в `directLEDValues=88` и не перехватывает кольца;
* при unmute single-LED командой возвращается белый цвет только затронутых номеров. Notify 5
  для восстановления не используется: тестами установлено, что он гасит панель.

Актуальная карта и анализ: [21-solo-led-map-and-restoration.ru.md](21-solo-led-map-and-restoration.ru.md).
Номер 2 подтверждён как индекс 12. Прежняя карта 0/8 была ошибочной.

Это исправляет исходную запись 2i2-полей `84/88` в Solo: на Solo эти адреса
являются соответственно индексом и началом массива LED-значений. Отдельный
ручной выбор *модели* не добавлен намеренно: выбор неверной модели отправил бы
команды по неподходящим аппаратным смещениям. Режим отображения можно выбрать
вручную в Settings → Mute display; безопасный вариант по умолчанию — `Auto`.

Физическая кнопка Direct включается в Settings флажком «Experimental: Direct
button». Это функция только Windows + Solo и выключена по умолчанию. Первое
нажатие ожидает 350 мс: одиночное нажатие возвращает прежнее состояние Direct
и меняет mute Windows, два нажатия переключают Direct. Прошивка успевает
кратко изменить Direct до того, как приложение получает событие, поэтому
короткое мигание Direct ожидаемо. Не удерживайте Direct три секунды при
включённой функции: прошивка использует это удержание для Combine Inputs, а
сигнал короткого нажатия нельзя надёжно отличить от начала удержания.
Возврат Direct выполняется не записью в читаемое поле состояния `264`: Solo
игнорирует такую запись. Используется проверенная на подключённом устройстве
последовательность parameter-buffer `217 = 0`, `216 = 0|1`, `DATA_NOTIFY(12)`.

Автоматический mute от положения ручки Gain не реализован. В проверенной
прошивке Solo 2.0.2417.0 нет доступного цифрового поля положения аналоговой
ручки; доступный meter показывает уровень сигнала, а не усиление. Подмена
одного другим приводила бы к ложному mute в тишине или при тихом источнике.

## Windows: нативная сборка

1. Установите Rust через [rustup](https://rustup.rs/) и MSVC-цель. Нужны
   Rust/Cargo не ниже версии, указанной в `crates/*/Cargo.toml`
   (`rust-version = 1.95`):

   ```powershell
   rustup toolchain install stable-x86_64-pc-windows-msvc
   rustup default stable-x86_64-pc-windows-msvc
   rustc --version
   cargo --version
   ```

2. Установите **Visual Studio Build Tools 2022** с workload «Desktop
   development with C++» и Windows 10/11 SDK. Это даёт линкер MSVC и SDK,
   которые требуются crate `windows`.

3. Установите Focusrite Control 2 и подключите Solo по USB. Программа
   использует его Windows-драйвер `FocusriteUsbSwRoot`; отдельный USB-драйвер
   или системная библиотека для Rust `base64` не нужны. `base64` — обычная
   зависимость Cargo и уже зафиксирована в `Cargo.lock`.

4. Из корня репозитория соберите и проверьте проект:

   ```powershell
   cargo fmt --all -- --check
   cargo test --locked --workspace
   cargo build --release --locked --workspace
   ```

   Исполняемые файлы появятся в `target\release\focusmute.exe` и
   `target\release\focusmute-cli.exe`. Для теста без установки запустите:

   ```powershell
   .\target\release\focusmute.exe
   .\target\release\focusmute-cli.exe devices
   ```

5. Чтобы создать MSI, установите WiX Toolset v3 и `cargo-wix`, затем выполните
   сборку на Windows:

   ```powershell
   cargo install cargo-wix --locked
   cargo wix -p focusmute
   ```

   Готовый пакет располагается в `target\wix\`.

## Linux: нативная сборка

Поддерживаются нативные Linux-сборки с PulseAudio либо PipeWire-Pulse и GTK 3.
Для Debian/Ubuntu установите инструменты и заголовки:

```bash
sudo apt-get update
sudo apt-get install build-essential pkg-config libpulse-dev libasound2-dev \
  libgtk-3-dev libxdo-dev libappindicator3-dev libssl-dev
rustup toolchain install stable
cargo test --locked --workspace
cargo build --release --locked --workspace
```

Исполняемые файлы: `target/release/focusmute` и
`target/release/focusmute-cli`. Для доступа обычного пользователя к USB
Focusrite установите правило из репозитория и переподключите устройство:

```bash
sudo install -m 644 dist/linux/99-focusrite.rules /etc/udev/rules.d/99-focusrite.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb
```

Для Debian-пакета после release-сборки:

```bash
cargo install cargo-deb --locked
cargo deb -p focusmute --no-build
```

Результат находится в `target/debian/`. На Wayland глобальные hotkey могут
быть ограничены композитором; используйте меню в трее, если сочетание не
регистрируется. Нативная поставка для macOS в проекте не предусмотрена:
backend мониторинга mute и tray собраны для Windows/Linux.

## Настройка и проверка Solo после сборки

1. Запустите приложение, откройте **Settings**, выберите `Русский` в Language
   и сохраните. Нажатие на поле Hotkey или Push-to-talk теперь переводит его в
   режим захвата: нажмите нужную клавишу или сочетание; Escape отменяет захват.
2. В Mute display оставьте `Auto`. На Solo он меняет номер выбранного канала,
   а для 2i2 сохраняет прежние номера входов. В выпадающем списке Solo отдельно показаны «Вход 1 — инструмент» и
   «Вход 2 — микрофон».
3. Нажмите глобальный hotkey и убедитесь, что номер 1, номер 2 или оба номера
   стали выбранного цвета согласно настройке. При снятии mute номера должны вернуться в белый цвет;
   Air и остальные LED не должны измениться.
4. Если нужна кнопка Direct, включите экспериментальный флажок и проверьте
   сначала одиночное, затем быстрое двойное нажатие. Клавиатурный hotkey и
   одиночное нажатие используют один и тот же Windows mute endpoint.

«Применить» сохраняет проверенную конфигурацию без закрытия окна; изменения
передаются работающему приложению после закрытия этого модального окна.
Настройки сохраняются в `%APPDATA%\Focusmute\config.toml` на Windows и в
`~/.config/focusmute/config.toml` на Linux. Эквивалент настроек в TOML:

```toml
[indicator]
mode = "numbers_blink"   # auto | numbers | numbers_blink
mute_color = "#FF0000"

[system]
language = "ru"          # en | ru

[hardware_button]
direct_button_enabled = false
direct_double_click_ms = 350
```

## Диагностика LED

Лаборатория LED сохранена в коде, но скрыта из интерфейса до следующей
итерации исследований. Её безопасные тесты охватывают номера 1 (индекс 4),
2 (индекс 12) и кратковременные halo. Bulk-тесты и тесты кнопок с неизвестным
восстановлением отключены. После старых bulk-тестов для полного восстановления
панели используйте переподключение USB.

Полная таблица и сценарии проверки находятся в документе 21 по ссылке выше.

## Выполненная проверка исходников

Новые регрессии проверяют индексы 4 и 12 вместо 0/8 для Solo, отсутствие bulk-записей
при mute/unmute и восстановлении, белый цвет после погашенной фазы,
остановку мигания после unmute и ограничение лабораторных команд моделью Solo.
Видимый результат следует сверять с панелью: чтение массива команд не
подтверждает фактические цвета светодиодов.
