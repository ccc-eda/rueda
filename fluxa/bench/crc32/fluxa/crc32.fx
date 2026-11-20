const CRC32_POLY: u32 = 0xEDB88320;

fn crc32_byte(crc: u32, byte: u8) -> u32 {
    let mut c: u32 = crc ^ (byte as u32);

    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }
    if (c & 1) != 0 {
        c = (c >> 1) ^ CRC32_POLY;
    } else {
        c = c >> 1;
    }

    c
}

task Crc32(x: in chan<u8>, y: out chan<u32>) {
    let mut crc: u32 @reg = 0xFFFFFFFF;

    pipeline(ii = 1) for byte in x {
        crc = crc32_byte(crc, byte);
    }

    y.send(crc ^ 0xFFFFFFFF);
    y.close();
}

test crc32_123456789 {
    let (tx, rx) = chan::<u8>(depth = 4);
    let (ty, ry) = chan::<u32>(depth = 1);
    spawn Crc32(rx, ty);

    for byte in [0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39] {
        tx.send(byte);
    }
    tx.close();

    assert ry.recv() == 0xCBF43926;
}

test crc32_binary {
    let (tx, rx) = chan::<u8>(depth = 4);
    let (ty, ry) = chan::<u32>(depth = 1);
    spawn Crc32(rx, ty);

    for byte in [0x00, 0xFF, 0x10, 0x20] {
        tx.send(byte);
    }
    tx.close();

    assert ry.recv() == 0xEE70D968;
}
