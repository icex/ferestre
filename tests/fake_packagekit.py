#!/usr/bin/env python3
"""A stand-in for PackageKit, on a private bus, for the GUI's install path.

It answers the calls the window makes -- CreateTransaction, SetHints, Resolve
and InstallPackages, with the signals PackageKit 1.x sends back -- and writes
each call to a log so a test can check what was asked. Nothing is installed:
an install can move one file into place instead, which is what a library
appearing looks like to the loader.

    start(home, log, install_move=(src, dst)) -> (address, processes)

starts a dbus-daemon of its own and this service on it. Point
DBUS_SYSTEM_BUS_ADDRESS at the address and GLib uses it as the system bus.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

NAME = "org.freedesktop.PackageKit"
PATH = "/org/freedesktop/PackageKit"
TRANSACTION = "org.freedesktop.PackageKit.Transaction"

# Values from PackageKit's lib/packagekit-glib2/pk-enum.h.
INFO_AVAILABLE = 2
EXIT_SUCCESS = 1
EXIT_FAILED = 2
ERROR_NOT_AUTHORIZED = 48

INTROSPECTION = f"""
<node>
  <interface name="{NAME}">
    <method name="CreateTransaction"><arg type="o" name="object_path" direction="out"/></method>
  </interface>
  <interface name="{TRANSACTION}">
    <method name="SetHints"><arg type="as" name="hints" direction="in"/></method>
    <method name="Resolve">
      <arg type="t" name="filter" direction="in"/>
      <arg type="as" name="packages" direction="in"/>
    </method>
    <method name="InstallPackages">
      <arg type="t" name="transaction_flags" direction="in"/>
      <arg type="as" name="package_ids" direction="in"/>
    </method>
    <signal name="Package">
      <arg type="u" name="info"/><arg type="s" name="package_id"/><arg type="s" name="summary"/>
    </signal>
    <signal name="ErrorCode"><arg type="u" name="code"/><arg type="s" name="details"/></signal>
    <signal name="Finished"><arg type="u" name="exit"/><arg type="u" name="runtime"/></signal>
    <signal name="Destroy"/>
  </interface>
</node>
"""

BUS_CONFIG = """<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <listen>{listen}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"""


def start(home, log, install_move=None, refuse=False):
    """A private bus with no service directory, so nothing on it is ever
    activated, and the fake PackageKit owning its name on it."""
    home = Path(home)
    config = home / "system-bus.conf"
    # In the temporary directory: a socket under a deep HOME can pass the 108
    # bytes a socket name may have.
    config.write_text(BUS_CONFIG.format(listen=f"unix:tmpdir={tempfile.gettempdir()}"))
    daemon = subprocess.Popen(["dbus-daemon", f"--config-file={config}", "--nofork",
                               "--nopidfile", "--print-address"],
                              stdout=subprocess.PIPE, text=True)
    address = daemon.stdout.readline().strip()
    if not address:
        daemon.terminate()
        raise RuntimeError("the private system bus did not start")
    command = [sys.executable, __file__, "--address", address, "--log", str(log)]
    if install_move:
        command += ["--install-move", str(install_move[0]), str(install_move[1])]
    if refuse:
        command.append("--refuse")
    service = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
    if service.stdout.readline().strip() != "ready":
        service.terminate()
        daemon.terminate()
        raise RuntimeError("the fake PackageKit did not take its name")
    return address, [service, daemon]


def serve(address, log, install_move, refuse):
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib

    connection = Gio.DBusConnection.new_for_address_sync(
        address,
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
        | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
        None,
        None,
    )
    node = Gio.DBusNodeInfo.new_for_xml(INTROSPECTION)
    daemon_info, transaction_info = node.interfaces
    count = [0]

    def record(**entry):
        with open(log, "a") as f:
            f.write(json.dumps(entry) + "\n")

    def emit(path, signal, signature=None, values=None):
        body = GLib.Variant(signature, values) if signature else None
        connection.emit_signal(None, path, TRANSACTION, signal, body)

    def later(path, signals):
        # After the method has returned, as the daemon does it.
        def send():
            for signal in signals:
                emit(path, *signal)
            return False
        GLib.timeout_add(50, send)

    def on_transaction(_conn, sender, path, _iface, method, parameters, invocation):
        args = parameters.unpack()
        record(method=method, path=path, args=list(args))
        if method == "SetHints":
            invocation.return_value(None)
        elif method == "Resolve":
            invocation.return_value(None)
            found = []
            for name in args[1]:
                # Twice, as the apt backend sends it, and a 32-bit copy, as a
                # multiarch Debian with Steam's i386 enabled reports it.
                for arch in ("i386", "amd64", "amd64"):
                    found.append(("Package", "(uss)",
                                  (INFO_AVAILABLE, f"{name};2.54.0-1;{arch};fixture", "fixture")))
            later(path, found + [("Finished", "(uu)", (EXIT_SUCCESS, 7)), ("Destroy",)])
        elif method == "InstallPackages":
            invocation.return_value(None)
            if refuse:
                later(path, [("ErrorCode", "(us)", (ERROR_NOT_AUTHORIZED, "Not authorized")),
                             ("Finished", "(uu)", (EXIT_FAILED, 3)), ("Destroy",)])
                return
            if install_move:
                os.replace(install_move[0], install_move[1])
            later(path, [("Finished", "(uu)", (EXIT_SUCCESS, 11)), ("Destroy",)])
        else:
            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)

    def on_daemon(_conn, sender, path, _iface, method, parameters, invocation):
        record(method=method, path=path, args=[])
        count[0] += 1
        transaction = f"{PATH}/fixture_{count[0]}"
        connection.register_object(transaction, transaction_info, on_transaction, None, None)
        invocation.return_value(GLib.Variant("(o)", (transaction,)))

    connection.register_object(PATH, daemon_info, on_daemon, None, None)
    loop = GLib.MainLoop()

    def acquired(*_):
        print("ready", flush=True)

    def lost(*_):
        loop.quit()

    Gio.bus_own_name_on_connection(connection, NAME, Gio.BusNameOwnerFlags.NONE, acquired, lost)
    loop.run()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--address", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--install-move", nargs=2)
    parser.add_argument("--refuse", action="store_true")
    options = parser.parse_args()
    serve(options.address, options.log, options.install_move, options.refuse)
