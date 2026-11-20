#!/usr/bin/env python3
"""Проверка трея лаунчера (задача 18) в невидимом сеансе.

    scripts/slint/headless.sh python3 scripts/slint/check-tray.py <hlsr> <папка> [--no-watcher]

Запускает StatusNotifierWatcher (kded6), лаунчер с данными в <папка>, жмёт F9
(XTest внутри виртуального Xwayland), проверяет значок и меню через D-Bus:
«Показать HLSR» возвращает окно на прежнее место и снимает значок, «Выход»
завершает процесс с 0. С --no-watcher трея нет: окно остаётся на экране, в
журнале — предупреждение, как в 0.4.7.
"""
import os
import subprocess
import sys
import time

import dbus
from Xlib import X, XK, display
from Xlib.ext import xtest

WATCHER = "org.kde.StatusNotifierWatcher"
exe, root = sys.argv[1], sys.argv[2]
no_watcher = "--no-watcher" in sys.argv[3:]
failures = []


def check(ok, what):
    print(("ок      " if ok else "ОШИБКА  ") + what, flush=True)
    if not ok:
        failures.append(what)


def wait(predicate, timeout=10.0):
    end = time.time() + timeout
    while time.time() < end:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    return predicate()


bus = dbus.SessionBus()
kded = None
if not no_watcher:
    kded = subprocess.Popen(["kded6"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    check(wait(lambda: bus.name_has_owner(WATCHER)), "StatusNotifierWatcher (kded6) на шине сеанса")


def items():
    try:
        watcher = bus.get_object(WATCHER, "/StatusNotifierWatcher")
        return [str(i) for i in watcher.Get(WATCHER, "RegisteredStatusNotifierItems",
                                            dbus_interface="org.freedesktop.DBus.Properties")]
    except dbus.DBusException:
        return []


d = display.Display()
root_window = d.screen().root


def launcher_window():
    clients = root_window.get_full_property(d.intern_atom("_NET_CLIENT_LIST"), X.AnyPropertyType)
    for wid in (clients.value if clients else []):
        w = d.create_resource_object("window", wid)
        try:
            if w.get_wm_class() == ("hlsr", "hlsr"):
                return w
        except Exception:
            pass
    return None


def mapped(w):
    try:
        return w.get_attributes().map_state == X.IsViewable
    except Exception:
        return False


def position(w):
    t = root_window.translate_coords(w, 0, 0)
    return t.x, t.y


def press_f9(w):
    w.set_input_focus(X.RevertToParent, X.CurrentTime)
    d.sync()
    time.sleep(0.2)
    code = d.keysym_to_keycode(XK.string_to_keysym("F9"))
    xtest.fake_input(d, X.KeyPress, code)
    xtest.fake_input(d, X.KeyRelease, code)
    d.sync()


def menu_click(item, label):
    service, _, path = item.partition("/")
    sni = bus.get_object(service, "/" + path)
    menu_path = sni.Get("org.kde.StatusNotifierItem", "Menu",
                        dbus_interface="org.freedesktop.DBus.Properties")
    menu = dbus.Interface(bus.get_object(service, menu_path), "com.canonical.dbusmenu")
    _, layout = menu.GetLayout(0, -1, ["label", "type"])
    entries = [(int(c[0]), str(c[1].get("label", "")), str(c[1].get("type", ""))) for c in layout[2]]
    print("        меню:", entries, flush=True)
    target = [i for i, text, _ in entries if text == label]
    if not target:
        return entries, False
    # Как Plasma: вызов с ответом; ответ должен прийти до снятия значка.
    menu.Event(target[0], "clicked", dbus.Int32(0, variant_level=1), dbus.UInt32(0))
    return entries, True


env = dict(os.environ, HLSR_DATA_DIR=f"{root}/data", HLSR_LOGS_DIR=f"{root}/logs")
os.makedirs(env["HLSR_DATA_DIR"], exist_ok=True)
proc = subprocess.Popen([exe], env=env)
try:
    w = wait(launcher_window, 15)
    check(w is not None and wait(lambda: mapped(w)), "окно лаунчера показано")
    check(items() == [], "значка в трее нет, пока окно на экране")
    start = position(w)

    if no_watcher:
        press_f9(w)
        time.sleep(1.5)
        check(mapped(w), "без трея F9 окно не прячет")
        proc.terminate()
        proc.wait(10)
        with open(f"{root}/logs/main.log", encoding="utf-8") as log:
            text = log.read()
        check("[tray] иконка не создана, окно остаётся на экране" in text,
              "в журнале: «иконка не создана, окно остаётся на экране»")
        print("итог:", "ошибок нет" if not failures else f"ошибок {len(failures)}")
        sys.exit(1 if failures else 0)

    press_f9(w)
    check(wait(lambda: not mapped(w), 5), "F9: окно спрятано")
    registered = wait(items, 5)
    check(len(registered) == 1, f"F9: значок в трее ({registered})")

    if registered:
        sni = bus.get_object(registered[0].partition("/")[0], "/" + registered[0].partition("/")[2])
        props = dbus.Interface(sni, "org.freedesktop.DBus.Properties")
        tooltip = props.Get("org.kde.StatusNotifierItem", "ToolTip")
        check(str(tooltip[2]) == "HLSR", f"подсказка значка «{tooltip[2]}»")
        pixmaps = props.Get("org.kde.StatusNotifierItem", "IconPixmap")
        check(len(pixmaps) == 1 and int(pixmaps[0][0]) > 0, "картинка значка передана")
        entries, clicked = menu_click(registered[0], "Показать HLSR")
        check([e[1] for e in entries] == ["Показать HLSR", "", "Выход"]
              and entries[1][2] == "separator", "меню: «Показать HLSR», разделитель, «Выход»")
        check(clicked and wait(lambda: mapped(w), 5), "«Показать HLSR»: окно вернулось")
        time.sleep(0.3)
        check(position(w) == start, f"окно на прежнем месте {start} → {position(w)}")
        check(wait(lambda: items() == [], 5), "«Показать HLSR»: значок снят")

    press_f9(w)
    registered = wait(items, 5)
    check(len(registered) == 1 and wait(lambda: not mapped(w), 5), "снова F9: окно в трее")
    if registered:
        sni = bus.get_object(registered[0].partition("/")[0], "/" + registered[0].partition("/")[2])
        sni.Activate(0, 0, dbus_interface="org.kde.StatusNotifierItem")
        check(wait(lambda: mapped(w), 5), "щелчок по значку: окно вернулось")
        check(wait(lambda: items() == [], 5), "щелчок по значку: значок снят")

    press_f9(w)
    registered = wait(items, 5)
    if registered:
        menu_click(registered[0], "Выход")
    try:
        code = proc.wait(10)
    except subprocess.TimeoutExpired:
        code = None
    check(code == 0, f"«Выход»: процесс завершился с кодом {code}")
finally:
    if proc.poll() is None:
        proc.kill()
    if kded:
        kded.terminate()

with open(f"{root}/logs/main.log", encoding="utf-8") as log:
    tray_lines = [line.rstrip() for line in log if "[tray]" in line]
check(tray_lines == [], f"нет предупреждений [tray] в журнале {tray_lines}")
print("итог:", "ошибок нет" if not failures else f"ошибок {len(failures)}")
sys.exit(1 if failures else 0)
