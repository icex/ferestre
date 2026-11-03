#!/usr/bin/env python3
"""Controller section focus and one visible selection outline per game tile."""
import argparse,ctypes,ctypes.util,os,subprocess,tempfile,time,struct,zlib
from pathlib import Path
import gui_smoke as s
from gui_launch import prepare
def png():
    def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    rows=(b'\0'+b'\x20\x40\x80'*600)*900
    return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',600,900,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(rows))+chunk(b'IEND',b'')
from controller_driver import Driver,focus,start_window_manager


def outline_bands(pid,button):
    x=ctypes.CDLL(ctypes.util.find_library('X11'));p=ctypes.c_void_p;u=ctypes.c_ulong
    x.XOpenDisplay.argtypes=[ctypes.c_char_p];x.XOpenDisplay.restype=p
    x.XDefaultRootWindow.argtypes=[p];x.XDefaultRootWindow.restype=u
    x.XGetImage.argtypes=[p,u,ctypes.c_int,ctypes.c_int,ctypes.c_uint,ctypes.c_uint,u,ctypes.c_int];x.XGetImage.restype=p
    x.XGetPixel.argtypes=[p,ctypes.c_int,ctypes.c_int];x.XGetPixel.restype=u
    x.XDestroyImage.argtypes=[p];x.XCloseDisplay.argtypes=[p]
    d=x.XOpenDisplay(None)
    try:
        origin=s.window_origin(pid);box=button.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)
        pixels=x.XGetImage(d,x.XDefaultRootWindow(d),origin[0]+box.x-15,origin[1]+box.y+box.height//3,45,1,u(-1).value,2)
        if not pixels:raise s.Failure('selection pixels unavailable')
        try:
            bands=[];inside=False
            for index in range(45):
                color=x.XGetPixel(pixels,index,0)&0xffffff
                r,g,b=color>>16,(color>>8)&255,color&255
                green=g>r+35 and g>b+35
                if green and not inside:bands.append(index-15)
                inside=green
            return bands
        finally:x.XDestroyImage(pixels)
    finally:x.XCloseDisplay(d)


def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',default='target/debug/ferestre-gui');args=p.parse_args()
    s.Atspi.init();gui=app=wm=None
    with tempfile.TemporaryDirectory(prefix='ferestre-controller-focus-') as tmp:
        h=Path(tmp);ids,_,pad,env=prepare(h,args.binary)
        if theme:=os.environ.get('FERESTRE_TEST_THEME_DIR'):
            (h/'.themes').mkdir();(h/'.themes'/Path(theme).name).symlink_to(theme,target_is_directory=True)
            env['GTK_THEME']=Path(theme).name
        for id in ids:
            image=h/f'cache/ferestre/catalog/images/{id}-cover-600.png';image.parent.mkdir(parents=True,exist_ok=True);image.write_bytes(png())
        try:
            wm=start_window_manager()
            with (h/'gui.log').open('wb') as log:gui=subprocess.Popen([h/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(gui.pid,time.monotonic()+20)
            def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
            frame=wait(lambda root:next(iter(s.find_all(root,role='frame',name='Ferestre')),None),'window')
            s.select_in_list(frame,'My Library');driver=Driver(app,pad)
            for fullscreen in (False,True):
                if fullscreen:s.click_named(frame,'Enter fullscreen');wait(lambda r:s.find_all(r,name='Exit fullscreen'),'fullscreen')
                driver.pulse(12);driver.seek('Launch Game 0 · Play');time.sleep(.35)
                bands=outline_bands(gui.pid,focus(app));print('Outline bands',fullscreen,bands,flush=True)
                s.check(len(bands)==1,'one green tile outline in '+('fullscreen' if fullscreen else 'desktop'))
                tile=focus(app)
                s.Atspi.generate_keyboard_event(0xff09,None,s.Atspi.KeySynthType.SYM)
                wait(lambda r:focus(r) is not None and 'More actions' in focus(r).get_name(),'keyboard focus enters the separate More control')
                time.sleep(.04)
                s.check(outline_bands(gui.pid,tile)==[],'moving focus inside a card removes its previous outer ring')
            s.click_named(frame,'Exit fullscreen');wait(lambda r:s.find_all(r,name='Enter fullscreen'),'desktop')
            s.select_in_list(frame,'Installed');driver.pulse(12);driver.seek('Install queue')
            driver.press(0,lambda root:s.find_all(root,name='Downloads page'),'open queue from library tabs')
            print('Queue focus:',focus(app).get_name() if focus(app) is not None else None,flush=True)
            wait(lambda root:focus(root) is not None and focus(root).get_name()=='Install queue','selected queue tab keeps focus')
            s.check(True,'Install queue selection never leaves focus on Installed')
            # Redraw the queue through a second navigation without accepting a
            # different tab merely because GTK assigned fallback focus to it.
            driver.seek('Installed');driver.press(0,lambda r:s.find_all(r,name='Installed page'),'Installed tab')
            wait(lambda r:focus(r) is not None and focus(r).get_name()=='Installed','Installed keeps its own focus')
            driver.seek('Install queue');driver.press(0,lambda r:s.find_all(r,name='Downloads page'),'queue again')
            wait(lambda r:focus(r) is not None and focus(r).get_name()=='Install queue','queue focus after repeated switches')
            driver.pulse(4);driver.seek('Install Queue')
            driver.press(0,lambda r:s.find_all(r,name='Downloads page'),'open queue from sidebar footer')
            wait(lambda r:focus(r) is not None and focus(r).get_name()=='Install queue','footer entry focuses the active queue tab')
            s.click_named(frame,'Close');s.check(gui.wait(timeout=10)==0,'focus fixture closes cleanly')
            print('PASS: section selection matches focus and game tiles have one outline',flush=True)
        except Exception:
            print((h/'gui.log').read_text(),flush=True)
            if app is not None:s.dump(app,'controller focus failure')
            raise
        finally:
            pad()
            if gui is not None and gui.poll() is None:gui.terminate();gui.wait(timeout=5)
            if wm is not None:wm.terminate();wm.wait(timeout=5)
if __name__=='__main__':main()
