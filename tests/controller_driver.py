"""Controller-only interaction through the fixture SDL adapter, no global input."""
import os
import subprocess
import time
import gui_smoke as s


def start_window_manager():
    """X11 focus assertions require a WM; external/private Wayland runners own theirs."""
    if os.environ.get('FERESTRE_TEST_WM') == '1':
        return None
    wm = subprocess.Popen(['openbox', '--sm-disable'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(.3)
    if wm.poll() is not None:
        raise RuntimeError('private test window manager did not start')
    return wm


def focus(root):
    return next((n for n,_ in s.walk(root) if n.get_state_set().contains(s.Atspi.StateType.FOCUSED)),None)


class Driver:
    def __init__(self, app, pad, kind=7):
        self.app,self.pad,self.kind=app,pad,kind
    def pulse(self, bit):
        self.pad(1<<bit,self.kind)
        time.sleep(.09)
        self.pad(0,self.kind)
        time.sleep(.14)
    def press(self, bit, predicate, what):
        self.pulse(bit)
        return s.wait_for(self.app,predicate,what,time.monotonic()+s.TIMEOUT)
    def seek(self, name):
        """Choose cardinal directions from allocations, exploring clipped controls."""
        attempts,trace={},[]
        for _ in range(30):
            try:
                current=s.wait_for(self.app,focus,'controller focus',time.monotonic()+s.TIMEOUT)
                current_name=current.get_name()
                ancestor=current
                while ancestor is not None:
                    if ancestor.get_name()==name:return current
                    ancestor=ancestor.get_parent()
                target=s.wait_for(self.app,lambda root:next((n for role in ('button','combo box','check box') for n in s.find_all(root,role=role,name=name)),None),'target '+name,time.monotonic()+s.TIMEOUT)
                c=current.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)
                t=target.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)
            except s.GLib.GError:
                time.sleep(.05)
                continue
            dx=(t.x+t.width/2)-(c.x+c.width/2)
            dy=(t.y+t.height/2)-(c.y+c.height/2)
            horizontal=14 if dx>=0 else 13
            vertical=12 if dy>=0 else 11
            choices=([horizontal,vertical] if abs(dx)>abs(dy) else [vertical,horizontal])+[11 if vertical==12 else 12,13 if horizontal==14 else 14]
            key=(current_name,c.x,c.y)
            attempt=attempts.get(key,0)
            attempts[key]=attempt+1
            direction=choices[attempt%len(choices)]
            trace.append((current_name,name,direction,(c.x,c.y,c.width,c.height),(t.x,t.y,t.width,t.height)))
            self.pulse(direction)
        print('CONTROLLER NAVIGATION TRACE',trace,flush=True)
        s.dump(self.app,'controller could not reach '+name)
        raise s.Failure('controller could not reach '+name)
