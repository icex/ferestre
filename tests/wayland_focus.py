"""Read focus in an isolated KWin session; never uses the user's compositor."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from gi.repository import Gio,GLib


def run_script(source):
    if os.environ.get('FERESTRE_TEST_PRIVATE_KWIN')!='1':raise RuntimeError('private KWin session required')
    with tempfile.TemporaryDirectory(prefix='ferestre-focus-probe-') as tmp:
        path=Path(tmp)/'probe.js';path.write_text(source)
        marker='ferestre-test-'+str(time.time_ns());base=['qdbus6','org.kde.KWin']
        ident=subprocess.check_output(base+['/Scripting','org.kde.kwin.Scripting.loadScript',str(path),marker],text=True).strip()
        try:subprocess.run(base+['/Scripting/Script'+ident,'org.kde.kwin.Script.run'],check=True,stdout=subprocess.DEVNULL)
        finally:subprocess.run(base+['/Scripting','org.kde.kwin.Scripting.unloadScript',marker],stdout=subprocess.DEVNULL)


def active_pid():
    bus=Gio.bus_get_sync(Gio.BusType.SESSION,None);found=[]
    xml='<node><interface name="io.github.icex.ferestre.FocusProbe"><method name="Result"><arg type="s" direction="in"/></method></interface></node>'
    info=Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0]
    def receive(conn,sender,path,iface,method,args,invocation):
        found.append(int(args.unpack()[0]));invocation.return_value(GLib.Variant('()',()))
    registration=bus.register_object('/FocusProbe',info,receive,None,None)
    try:
        run_script('callDBus('+json.dumps(bus.get_unique_name())+',"/FocusProbe","io.github.icex.ferestre.FocusProbe","Result",String(workspace.activeWindow ? workspace.activeWindow.pid : 0));')
        until=time.monotonic()+2
        while not found and time.monotonic()<until:GLib.MainContext.default().iteration(False);time.sleep(.005)
        return found[0] if found else None
    finally:bus.unregister_object(registration)


def focus_fixture(pid):
    # Establish the initial game foreground when Wayland rejects the fixture's
    # own presentation. This is never called during a product Resume check.
    run_script('for (const w of workspace.windowList()) {if (w.pid === '+str(int(pid))+' && w.normalWindow) {w.minimized=false;workspace.activeWindow=w;break;}}')
