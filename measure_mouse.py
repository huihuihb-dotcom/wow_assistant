import serial
import serial.tools.list_ports
import time
import ctypes
from ctypes import wintypes

user32 = ctypes.windll.user32
user32.SetProcessDPIAware() # 声明物理 DPI

def get_pt():
    pt = wintypes.POINT()
    user32.GetCursorPos(ctypes.byref(pt))
    return pt.x, pt.y

# 找 COM3
ports = [p.device for p in serial.tools.list_ports.comports() if "COM3" in p.device or "2E8A" in (p.hwid or "")]
port_name = ports[0] if ports else "COM3"

ser = serial.Serial(port_name, 115200, timeout=1.0)
time.sleep(0.5)

print("Starting measurement from current cursor pos...")
x0, y0 = get_pt()
print(f"Initial cursor: ({x0}, {y0})")

# 发送 M,100,100
ser.write(b"M,100,100\n")
ser.flush()
line = ser.readline().decode().strip()
time.sleep(0.2)
x1, y1 = get_pt()
print(f"After M,100,100 -> Cursor: ({x1}, {y1}), Actual delta: dx={x1 - x0}, dy={y1 - y0}, expected: (100, 100)")

# 发送 HM,100,100
ser.write(b"HM,100,100,2.0\n")
ser.flush()
line = ser.readline().decode().strip()
time.sleep(0.5)
x2, y2 = get_pt()
print(f"After HM,100,100,2.0 -> Cursor: ({x2}, {y2}), Actual delta: dx={x2 - x1}, dy={y2 - y1}, expected: (100, 100)")

ser.close()
