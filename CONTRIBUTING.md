# Как помочь HLSR

*English below.*

Спасибо, что заглянули! Вот как прислать исправление, чтобы его можно было
быстро принять.

## 🔵 Сначала issue

Нашли ошибку или есть идея — откройте issue (там есть формы «Ошибка» и
«Предложение»). Если задумали что-то большое, например новый экран, новую игру
или новую зависимость, сначала обсудите: так меньше шансов, что придётся
переделывать.

Уязвимости в issue не пишите. Как сообщить о них закрыто, сказано в
[SECURITY.md](SECURITY.md).

Сервер hlsr.pro закрытый. Если для исправления нужно что-то поменять на
сервере, опишите это в issue, серверную часть сделаем сами.

## 🔵 Как собрать

Что поставить и какие команды запускать, написано в [README.ru.md](README.ru.md)
(раздел «Сборка»). Коротко:

```bash
cargo run -p hlsr-app            # с HLSR_DATA_DIR и HLSR_LOGS_DIR во временных папках
cargo test -p <крейт>            # тесты того, что вы трогали
```

На Windows до checkout выполните `git config --global core.autocrlf false`:
фикстуры сравниваются байт в байт.

## 🔵 Pull request

Один PR — одно изменение, маленькие принимаются быстрее. Перед отправкой
прогоните:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Если поменялся `Cargo.lock`, обновите список крейтов в
`build/THIRD-PARTY-NOTICES.txt` скриптом `scripts/slint/crate-notices.py`
(как его запускать, написано в начале скрипта).

Новую логику присылайте с тестом. Фикстуры в `tests/fixtures` руками ради
зелёного теста не правьте. Если поведение меняется специально, объясните
это в PR. И заполните шаблон PR: что поменялось для игрока и как вы это
проверили.

## 🔵 Как пишем код

- Язык проекта русский: комментарии, сообщения журнала, тексты ошибок и
  заголовки коммитов пишем по-русски. Заголовок коммита выглядит так:
  `Область: что сделано`, например `Доставка: повтор загрузки после обрыва`.
- Тексты интерфейса лежат в `crates/hlsr-ui/locales/{ru,en}.json`. Новую
  строку добавляйте в оба файла.
- Экраны в `crates/hlsr-gpui` только рисуют состояние контроллеров из
  `crates/hlsr-ui` и передают им действия игрока, логики в них нет.
- Процессы, реестр, диалоги и сеть спрятаны за трейтами в `ports.rs`, в тестах
  вместо них подделки. Тесты не трогают настоящие `%APPDATA%\hlsr`,
  `~/.config/hlsr` и реестр.
- Новую зависимость добавляйте, только если без неё совсем никак, и её
  лицензия должна быть в списке `deny.toml`.

## 🔵 Секреты и личные данные

Не кладите в репозиторий ключи, токены, пароли, `.env` с настоящими
значениями, сборки игр, архивы, журналы и чужие данные. В тестах только
явно ненастоящие значения. Ключи подписи есть только у сопровождающих, для
сборки и тестов они не нужны.

## 🔵 Лицензия вклада

Присылая PR, вы соглашаетесь, что ваш код распространяется по лицензии
проекта, GNU GPL версии 3 ([LICENSE](LICENSE)), и что у вас есть право его
так отдать. Никаких отдельных соглашений (CLA) подписывать не нужно.

---

## English

- Open an issue first, and talk about big changes before you write the code.
  Report vulnerabilities privately ([SECURITY.md](SECURITY.md)).
- One change per pull request. Run `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` and `cargo deny check` first.
- The project language is Russian: code comments, log messages, error texts
  and commit subjects (`Area: what was done`) are in Russian. If you don't
  write Russian, write in English and we'll help.
- Never commit secrets, game builds or anyone's personal data. Tests use
  obviously fake values.
- By sending a pull request you agree that your code is licensed under the
  project license, GPL-3.0-only, and that you have the right to license it.
  There is no CLA.
