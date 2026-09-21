//! Small built-in translation catalogue. Configuration values stay language
//! neutral; only text shown to the user is translated.

pub fn tr(language: &str, key: &str) -> &'static str {
    if !language.eq_ignore_ascii_case("ru") {
        return english(key);
    }
    match key {
        "settings" => "Настройки",
        "mute_indicator" => "Индикация mute",
        "keyboard" => "Клавиатура",
        "hotkey" => "Горячая клавиша",
        "push_to_talk" => "Нажми и говори",
        "record" => "Записать",
        "press_keys" => "Нажмите клавишу или сочетание. Escape — отмена.",
        "sound" => "Звук",
        "system" => "Система",
        "language" => "Язык",
        "save" => "Сохранить",
        "cancel" => "Отмена",
        "muted" => "Микрофон выключен",
        "live" => "Микрофон включён",
        "toggle_mute" => "Переключить mute",
        "reconnect" => "Переподключить устройство",
        "resync" => "Синхронизировать устройство",
        "connect_device" => "Подключите Scarlett 4th Gen",
        "quit" => "Выход",
        "advanced" => "Дополнительно",
        "mute_color" => "Цвет mute",
        "mute_inputs" => "Входы mute",
        "mute_display" => "Индикация mute",
        "auto" => "Автоматически",
        "numbers" => "Только номера",
        "numbers_blink" => "Мигание при mute",
        "solo_number_note" => {
            "Solo: mute подсвечивает выбранный номер канала: 1 (LED 4), 2 (LED 12) или оба. После unmute номера возвращаются в белый цвет."
        }
        "mute_display_tip" => {
            "Solo: mute использует только номера выбранных каналов: 1 — LED 4, 2 — LED 12. Мигание при mute не зависит от уровня звука. 2i2 использует прежнюю карту номеров."
        }
        "solid_halos" => "Сплошные кольца уровня",
        "blink_on_talk" => "Мигание при разговоре",
        "sensitivity" => "Чувствительность",
        "sound_feedback" => "Звуковой отклик",
        "mute_sound" => "Звук выключения",
        "unmute_sound" => "Звук включения",
        "clear" => "Очистить",
        "browse" => "Обзор...",
        "built_in" => "(встроенный)",
        "start_windows" => "Запускать вместе с Windows",
        "start_system" => "Запускать вместе с системой",
        "notifications" => "Уведомления рабочего стола",
        "direct_button" => "Экспериментально: Direct — одно нажатие переключает mute, два — Direct",
        "direct_tip" => {
            "Только Solo. Сначала устройство кратко меняет Direct, затем FocusMute распознаёт жест."
        }
        "log_level" => "Уровень журнала",
        "webhooks" => "Вебхуки",
        "browser_sync" => "Синхронизация с браузером",
        "about" => "О программе",
        "volume" => "Громкость",
        "play" => "Воспроизвести",
        "on_mute_url" => "URL при mute",
        "on_unmute_url" => "URL при unmute",
        "body" => "Тело",
        "suppress_sound" => "Не воспроизводить звук при mute/unmute",
        "meeting_mute" => "Переключать mute конференции из FocusMute",
        "port" => "Порт",
        "source" => "Исходный код",
        "about_description" => {
            "Управление mute через горячие клавиши для Focusrite Scarlett 4th Gen"
        }
        "main" => "Основные",
        "led_lab" => "Проверка LED",
        "led_lab_title" => "Лаборатория индикаторов Solo",
        "led_lab_intro" => {
            "Карта из 20-ledtest.md. Доступны номера 1 (4), 2 (12) и кратковременные тесты halo. Тесты кнопок и bulk отключены: их цвета нельзя достоверно восстановить из массива команд. Цвет берётся из основных настроек. Снимок — буфер команд, не фактические цвета. После старых bulk-тестов восстановите панель переподключением USB."
        }
        "led_snapshot" => "Снимок состояния",
        "led_reset" => "Вернуть выбранные номера в белый",
        "led_index" => "Индекс LED / slot",
        "led_single_test" => "Проверить один LED",
        "led_next_test" => "Следующий LED",
        "led_bulk_mode" => "Режим bulk-проверки",
        "led_bulk_test" => "Проверить bulk-slot",
        "led_lab_result" => "Результат команды",
        "apply" => "Применить",
        "input_levels" => "Уровни активных входов",
        "input_level_tip" => {
            "Живой уровень сигнала с выбранных входов Scarlett. Он показывает сигнал на входе, а не состояние mute Windows."
        }
        "meter_unavailable" => "Нет данных от устройства",
        "blink_tip" => {
            "Когда Windows mute включён и сигнал выбранного входа превышает порог, индикатор mute мигает. Это предупреждает, что вы говорите при выключенном микрофоне."
        }
        _ => english(key),
    }
}

fn english(key: &str) -> &'static str {
    match key {
        "settings" => "Settings",
        "mute_indicator" => "Mute Indicator",
        "keyboard" => "Keyboard",
        "hotkey" => "Hotkey",
        "push_to_talk" => "Push-to-talk",
        "record" => "Record",
        "press_keys" => "Press a key or key combination. Escape cancels.",
        "sound" => "Sound",
        "system" => "System",
        "language" => "Language",
        "save" => "Save",
        "cancel" => "Cancel",
        "muted" => "Muted",
        "live" => "Live",
        "toggle_mute" => "Toggle Mute",
        "reconnect" => "Reconnect device",
        "resync" => "Re-sync device",
        "connect_device" => "Connect a Scarlett 4th Gen device",
        "quit" => "Quit",
        "advanced" => "Advanced",
        "mute_color" => "Mute Color",
        "mute_inputs" => "Mute Inputs",
        "mute_display" => "Mute display",
        "auto" => "Auto",
        "numbers" => "Numbers only",
        "numbers_blink" => "Blink while muted",
        "solo_number_note" => {
            "Solo: mute lights the selected channel number: 1 (LED 4), 2 (LED 12), or both. Unmute restores those numbers to white."
        }
        "mute_display_tip" => {
            "Solo: mute uses only the selected channel numbers: 1 is LED 4 and 2 is LED 12. Mute blinking is independent of input level. 2i2 retains its number map."
        }
        "solid_halos" => "Solid level halos",
        "blink_on_talk" => "Blink on talk",
        "sensitivity" => "Sensitivity",
        "sound_feedback" => "Sound feedback",
        "mute_sound" => "Mute Sound",
        "unmute_sound" => "Unmute Sound",
        "clear" => "Clear",
        "browse" => "Browse...",
        "built_in" => "(built-in)",
        "start_windows" => "Start with Windows",
        "start_system" => "Start with System",
        "notifications" => "Desktop notifications",
        "direct_button" => {
            "Experimental: Direct button — one press toggles mute, two presses toggle Direct"
        }
        "direct_tip" => {
            "Solo only. Direct changes briefly before FocusMute can classify the gesture."
        }
        "log_level" => "Log level",
        "webhooks" => "Webhooks",
        "browser_sync" => "Browser sync",
        "about" => "About",
        "volume" => "Volume",
        "play" => "Play",
        "on_mute_url" => "On mute URL",
        "on_unmute_url" => "On unmute URL",
        "body" => "Body",
        "suppress_sound" => "Suppress sound on mute/unmute",
        "meeting_mute" => "Let FocusMute mute/unmute the meeting",
        "port" => "Port",
        "source" => "Source",
        "about_description" => "Hotkey mute control for Focusrite Scarlett 4th Gen interfaces",
        "main" => "Main",
        "led_lab" => "LED Lab",
        "led_lab_title" => "Solo indicator laboratory",
        "led_lab_intro" => {
            "Map from 20-ledtest.md. Tests are available for numbers 1 (4), 2 (12), and transient halos. Button and bulk tests are disabled: the command buffer cannot reliably restore their colours. Test colour comes from Main settings. A snapshot is a command buffer, not actual panel colours. Reconnect USB to recover from earlier bulk tests."
        }
        "led_snapshot" => "Read state snapshot",
        "led_reset" => "Restore selected numbers to white",
        "led_index" => "LED / slot index",
        "led_single_test" => "Test one LED",
        "led_next_test" => "Next LED",
        "led_bulk_mode" => "Bulk test mode",
        "led_bulk_test" => "Test bulk slot",
        "led_lab_result" => "Command result",
        "apply" => "Apply",
        "input_levels" => "Active input levels",
        "input_level_tip" => {
            "Live signal from selected Scarlett inputs. This is input signal, not the Windows mute state."
        }
        "meter_unavailable" => "No device meter data",
        "blink_tip" => {
            "When Windows mute is on and a selected input exceeds the threshold, the mute indicator blinks to warn that you are talking while muted."
        }
        _ => "",
    }
}
