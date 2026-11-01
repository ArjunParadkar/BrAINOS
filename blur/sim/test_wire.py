"""Golden vectors shared with os/blur/tests/blur_suite.rs: the two
implementations of the nerve formats must produce the same bytes."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wire

def test_setpoint_golden():
    f = wire.setpoint_encode(0, 0x11, 'gimbal2204', 2, wire.SP_ENABLE, -1.0622, 0.25, 20.0, 0.5)
    assert f.id == 0x111 and f.data == bytes([0x02, 0x01, 0x67, 0xEF, 0xFA, 0x00, 0x50, 0x32]), f

def test_feedback_golden():
    f = wire.feedback_encode(1, 0x08, 'esc2806', 0, wire.ST_ENC_OK | wire.ST_CALIBRATED, 1.5351, -3.21, 12.34)
    assert f.id == 0x188 and f.data == bytes([0x00, 0x42, 0xFC, 0x17, 0xBF, 0xFE, 0xD2, 0x04]), f

def test_frame_pack_roundtrip():
    f = wire.Frame.make(2, wire.FC_CONFIG, 0x12, bytes(range(8)))
    g = wire.Frame.unpack(f.pack())
    assert (g.bus, g.id, g.data) == (f.bus, f.id, f.data)

if __name__ == '__main__':
    for k, v in list(globals().items()):
        if k.startswith('test_'): v(); print('ok', k)
