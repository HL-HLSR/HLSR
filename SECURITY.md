# Безопасность / Security

*English below.*

## 🔵 Какие версии исправляем

Исправления безопасности выходят в последнем выпуске и в ветке `main`.
Старые версии обновляются до последней через сам лаунчер.

## 🔵 Как сообщить об уязвимости

Не пишите о дыре в открытые issue, pull request или Discord, пока её не
закрыли. Есть закрытая форма GitHub: вкладка **Security** этого репозитория,
кнопка **Report a vulnerability**.

Напишите:

- какая версия и где (Windows или Linux);
- как повторить и что получает атакующий;
- как исправить, если есть идея.

Мы ответим, разберёмся, сделаем исправление и договоримся с вами, когда о нём
рассказать. Денежных наград (bug bounty) нет.

## 🔵 Что сюда относится

Лаунчер и обновлялка из этого репозитория: скачивание и проверка подписей,
установка, патчи и починка, запуск процессов, IPC, локальные файлы и
настройки, онлайн-клиент.

Сервер и сайт hlsr.pro закрыты, но об их уязвимостях можно писать так же.
Проверяйте только на своём аккаунте, без DoS и не трогая чужие данные.

Сюда не относятся сборки игр и моды (о них пишите их авторам) и читы, которые
не используют дыры в лаунчере или сервере.

## 🔵 Секреты

Боевые ключи подписи, токены выпуска, сертификаты, доступы к выкладке, файлы
`.env` и настоящие данные игроков в репозиторий не попадают. В фикстурах тестов
только явно ненастоящие значения. Если нашли в репозитории или его истории
настоящий секрет, сообщите о нём тем же закрытым способом.

---

## English

Security fixes go to the latest release and the `main`
branch. Older versions update to the latest one through the launcher.

Don't post an unfixed vulnerability in a public issue, pull
request or Discord. Use GitHub's private form: the **Security** tab of this
repository, **Report a vulnerability**. Tell us the version and platform, how
to reproduce it and what an attacker gets, and a fix if you have one. We will
reply, look into it, prepare a fix and agree with you on when to disclose it.
There is no bug bounty.

This covers the launcher and updater in this repository: downloads and signature
checks, install, patch and repair, process launch, IPC, local files and
settings, the online client. The hlsr.pro server and website are closed
source, but you can report their vulnerabilities the same way. Test only on
your own account, without denial of service and without touching other
players' data. Game builds and mods (report those to their authors) and cheats
that don't exploit the launcher or the server are out of scope.

Production signing keys, release tokens, certificates, deploy
credentials, `.env` files and real player data are never committed. Test
fixtures use obviously fake values. If you find a real secret in this
repository or its history, report it privately the same way.
