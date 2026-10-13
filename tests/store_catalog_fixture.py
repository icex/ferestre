#!/usr/bin/env python3
"""Account-free catalog IPC fixture. No HTTP or credentials are used."""
import os, socket, struct, sys, threading, time
from pathlib import Path
root=Path(sys.argv[1]);root.mkdir(mode=0o700,parents=True,exist_ok=True)
def u(n): return struct.pack('<I',n)
def text(s):
 b=s.encode();return u(len(b))+b
def product(index):
 b=u(2)
 for s in [f'9CATALOG000{index}',f'café product {index}','description','en-US',f'offer-{index}']: b+=text(s)
 b+=u(1)
 for s in ['0010','SKU title','SKU description','en-US']:b+=text(s)
 b+=u(0)+u(0)+u(1)+text('availability')+struct.pack('<ff',3,2)+text('USD')+struct.pack('<Q',0)
 b+=u(1)
 for s in ['https://example.invalid/image','caption','BoxArt']:b+=text(s)
 return b+u(128)+u(256)
def exact(s,n):
 b=b''
 while len(b)<n:
  chunk=s.recv(n-len(b))
  if not chunk:raise EOFError
  b+=chunk
 return b
def run(conn):
 with conn:
  try:
   magic,size=struct.unpack('<II',exact(conn,8));assert magic==0x58445343 and size<=24000
   req=exact(conn,size);mode=struct.unpack('<I',req[4:8])[0]
   if b'9BADDECODE00' in req:reply=u(1)+u(0)+u(1)+product(0)[:-3]
   elif b'9FAILQUERY00' in req:reply=u(1)+u(0x80004005)+u(0)
   elif b'9TRUNCATED00' in req:
    conn.sendall(b'CSDX'+u(24)+u(1));return
   else:
    time.sleep(.5)
    reply=u(1)+u(0)+u(3 if mode else 1)
    if mode:reply+=b''.join(product(i) for i in range(3))
    else:reply+=product(0).replace(b'9CATALOG0000',b'9N1J3D5WNSKP')
   conn.sendall(b'CSDX'+u(len(reply))+reply)
  except (EOFError,BrokenPipeError):pass
sock=socket.socket(socket.AF_UNIX);sock.bind(str(root/'xodus.sock'));os.chmod(root/'xodus.sock',0o600);sock.listen()
(root/'ready').touch()
while True:
 conn,_=sock.accept();threading.Thread(target=run,args=(conn,),daemon=True).start()
