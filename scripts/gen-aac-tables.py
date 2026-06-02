#!/usr/bin/env python3
# Regenerates src/m4a/aac_tables.rs from FFmpeg's aactab.c (LGPL source; the
# tables are the factual AAC Huffman codebooks + scalefactor-band offsets from
# ISO/IEC 14496-3). File-to-file transform, no manual transcription.
import os, re, sys, urllib.request

URL = "https://raw.githubusercontent.com/FFmpeg/FFmpeg/master/libavcodec/aactab.c"
cache = "/tmp/aactab.c"
if not os.path.exists(cache):
    urllib.request.urlretrieve(URL, cache)
src = open(cache).read()

def grab(name):
    m = re.search(r'\b' + re.escape(name) + r'\s*\[[^\]]*\]\s*=\s*\{(.*?)\}', src, re.S)
    if not m:
        sys.exit('missing ' + name)
    return [int(v, 0) for v in re.findall(r'0x[0-9a-fA-F]+|\d+', m.group(1))]

out = ['#![allow(clippy::all)]', '#![allow(dead_code)]']
out.append('pub const AAC_SPEC_SIZES: [usize; 11] = [81, 81, 81, 81, 81, 81, 64, 64, 169, 169, 289];')

sizes = [81,81,81,81,81,81,64,64,169,169,289]
cn, bn = [], []
for cb in range(1, 12):
    codes, bits = grab(f'codes{cb}'), grab(f'bits{cb}')
    assert len(codes) == sizes[cb-1] and len(bits) == sizes[cb-1]
    out.append(f'static CODES{cb}: [u16; {sizes[cb-1]}] = {codes!r};')
    out.append(f'static BITS{cb}: [u8; {sizes[cb-1]}] = {bits!r};')
    cn.append(f'CODES{cb}'); bn.append(f'BITS{cb}')
out.append('pub static AAC_SPEC_CODES: [&[u16]; 11] = [' + ', '.join('&'+n for n in cn) + '];')
out.append('pub static AAC_SPEC_BITS: [&[u8]; 11] = [' + ', '.join('&'+n for n in bn) + '];')
out.append(f'pub static AAC_SCF_CODES: [u32; 121] = {grab("ff_aac_scalefactor_code")!r};')
out.append(f'pub static AAC_SCF_BITS: [u8; 121] = {grab("ff_aac_scalefactor_bits")!r};')

map1024 = [96,96,64,48,48,32,24,24,16,16,16,8,8]
map128  = [96,96,96,48,48,48,24,24,16,16,16,8,8]
emitted = set()
def emit(win, rate):
    name = f'SWB_{win}_{rate}'
    if name not in emitted:
        emitted.add(name)
        vals = grab(f'swb_offset_{win}_{rate}')
        out.append(f'static {name}: [u16; {len(vals)}] = {vals!r};')
    return name
r1024 = [emit(1024, r) for r in map1024]
r128 = [emit(128, r) for r in map128]
out.append('pub static AAC_SWB_OFFSET_1024: [&[u16]; 13] = [' + ', '.join('&'+r for r in r1024) + '];')
out.append('pub static AAC_SWB_OFFSET_128: [&[u16]; 13] = [' + ', '.join('&'+r for r in r128) + '];')
out.append(f'pub static AAC_NUM_SWB_1024: [u8; 13] = {grab("ff_aac_num_swb_1024")!r};')
out.append(f'pub static AAC_NUM_SWB_128: [u8; 13] = {grab("ff_aac_num_swb_128")!r};')
out.append(f'pub static AAC_TNS_MAX_1024: [u8; 13] = {grab("ff_tns_max_bands_1024")!r};')
out.append(f'pub static AAC_TNS_MAX_128: [u8; 13] = {grab("ff_tns_max_bands_128")!r};')

dst = os.path.join(os.path.dirname(__file__), '..', 'src', 'm4a', 'aac_tables.rs')
open(dst, 'w').write('\n'.join(out) + '\n')
print('wrote', dst)
