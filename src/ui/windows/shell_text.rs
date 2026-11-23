//! Native-shell copy keyed by the shared application language.
use super::locale::Language;

// Each key has both translations. Menu commands and IPC identifiers stay outside this table.
macro_rules! shell_text {
    ($($key:ident => $en:literal, $ru:literal;)*) => {
        #[derive(Clone, Copy)]
        pub(super) enum Text {
            $($key,)*
        }

        impl Text {
            pub(super) fn get(self, language: Language) -> &'static str {
                match (self, language) {
                    $((Self::$key, Language::En) => $en,
                    (Self::$key, Language::Ru) => $ru,)*
                }
            }
        }
    };
}

shell_text! {
    InstalledStartupFailed => "Prismestra couldn’t start.", "Не удалось запустить Prismestra.";
    StandaloneStartupFailed => "Prismestra couldn’t start", "Не удалось запустить программу";
    OpenPrismestra => "Open Prismestra", "Открыть Prismestra";
    CheckForUpdates => "Check for updates…", "Проверить обновления…";
    UpdateAvailable => "Update available", "Доступно обновление";
    CheckForUpdatesAutomatically => "Check for updates automatically", "Автоматически проверять обновления";
    StopOutputs => "Stop outputs", "Остановить работу";
    ExitMenu => "Exit Prismestra", "Выйти";
    ApplySavedScene => "Apply a saved scene", "Включить сохранённую сцену";
    ExitTitle => "Exit Prismestra", "Выход из Prismestra";
    CloseEditorTitle => "Close editor", "Закрытие редактора";
    EditsWillBeLost => "Unapplied settings will be lost.", "Неприменённые значения настроек будут потеряны.";
    ConfirmExitDiscard => "Unapplied settings will be lost. Exit Prismestra?", "Неприменённые правки будут потеряны. Выйти?";
    ReturnToEdits => "Return to edits", "Вернуться к правкам";
    ExitWithoutApplying => "Exit without applying", "Выйти без применения";
    CloseWithoutApplying => "Close without applying", "Закрыть без применения";
    SaveChangedProfile => "Save changes to the profile?", "Сохранить изменённый профиль?";
    UnsavedChanges => "Unsaved changes", "Есть несохранённые изменения";
    ResolveEditsBeforeExit => "Apply or discard your changes before exiting.", "Примените или сбросьте правки перед выходом.";
    ResolveEditsBeforeClose => "Apply or discard your changes before closing the editor.", "Примените или сбросьте правки параметров перед закрытием редактора.";
    TrayUnavailable => "The Prismestra tray icon is not available yet. The editor will stay open while Prismestra retries.", "Значок Prismestra пока недоступен. Приложение продолжает работу; редактор останется открытым, пока значок не появится.";
    TraySceneUnavailable => "To switch scenes from the tray, save or discard your draft and close the editor.", "Переключение из трея недоступно. Закройте редактор без несохранённого черновика.";
    TraySceneChanged => "The scene or draft changed. Reopen the tray menu.", "Сцена или черновик изменились. Откройте меню заново.";
    ModernEditorUnavailable => "The modern editor is unavailable", "Новый интерфейс недоступен";
    ModernEditor => "Modern editor", "Новый интерфейс";
    Scene => "Scene", "Сцена";
    Devices => "Devices", "Устройства";
    Connections => "Connections", "Подключения";
    Schedule => "Schedule", "Расписание";
    ScreenLighting => "Screen lighting", "Подсветка экрана";
    Temperatures => "Temperatures", "Температуры";
    Sensors => "Sensors", "Датчики";
    EnableSchedule => "Enable schedule", "Включить расписание";
    PauseSchedule => "Pause schedule", "Приостановить расписание";
    PrimaryLighting => "Primary lighting", "Основной свет";
    SecondaryLighting => "Secondary lighting", "Дополнительный свет";
    LightsOffTime => "Turn lights off", "Выключение подсветки";
    TimeZone => "Time zone", "Часовой пояс";
    ApplySchedule => "Apply schedule", "Применить расписание";
    DiscardChanges => "Discard changes", "Отменить изменения";
    NewScene => "New scene", "Новая сцена";
    SceneFileFilter => "Prismestra scene (*.json)\0*.json\0\0", "Сцена Prismestra (*.json)\0*.json\0\0";
    OpenProfile => "Open…", "Открыть…";
    SaveProfile => "Save…", "Сохранить…";
    ExportZip => "Export ZIP…", "Экспорт ZIP…";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_language_distinguishes_close_and_exit() {
        assert_eq!(
            Text::ExitWithoutApplying.get(Language::En),
            "Exit without applying"
        );
        assert_eq!(
            Text::CloseWithoutApplying.get(Language::Ru),
            "Закрыть без применения"
        );
    }
}
