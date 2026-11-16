const PROB_BITS: u32 = 11;
const PROB_INIT: u16 = 1 << (PROB_BITS - 1);
const MOVE_BITS: u32 = 5;
const TOP: u32 = 1 << 24;
const HEADER_BYTES: usize = 13;
const MIN_DICTIONARY: u32 = 1 << 12;
const STATES: usize = 12;
const POS_STATES_MAX: usize = 1 << 4;
const LEN_TO_POS_STATES: usize = 4;
const END_POS_MODEL: u32 = 14;
const FULL_DISTANCES: usize = 1 << (END_POS_MODEL >> 1);
const ALIGN_BITS: u32 = 4;
const MATCH_MIN: usize = 2;
const END_MARKER: u32 = u32::MAX;

struct Corrupt;

struct Ranges<'a> {
    input: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> Ranges<'a> {
    fn new(input: &'a [u8], start: usize) -> Result<Self, Corrupt> {
        let mut rc = Self {
            input,
            pos: start,
            range: u32::MAX,
            code: 0,
        };
        if rc.byte()? != 0 {
            return Err(Corrupt);
        }
        for _ in 0..4 {
            rc.code = (rc.code << 8) | u32::from(rc.byte()?);
        }
        if rc.code == rc.range {
            return Err(Corrupt);
        }
        Ok(rc)
    }

    fn byte(&mut self) -> Result<u8, Corrupt> {
        let byte = *self.input.get(self.pos).ok_or(Corrupt)?;
        self.pos += 1;
        Ok(byte)
    }

    fn normalize(&mut self) -> Result<(), Corrupt> {
        if self.range < TOP {
            self.range <<= 8;
            self.code = (self.code << 8) | u32::from(self.byte()?);
        }
        Ok(())
    }

    fn bit(&mut self, probs: &mut [u16], index: usize) -> Result<usize, Corrupt> {
        let prob = u32::from(probs[index]);
        let bound = (self.range >> PROB_BITS) * prob;
        let bit = if self.code < bound {
            probs[index] = (prob + (((1 << PROB_BITS) - prob) >> MOVE_BITS)) as u16;
            self.range = bound;
            0
        } else {
            probs[index] = (prob - (prob >> MOVE_BITS)) as u16;
            self.code -= bound;
            self.range -= bound;
            1
        };
        self.normalize()?;
        Ok(bit)
    }

    fn direct(&mut self, count: u32) -> Result<u32, Corrupt> {
        let mut value = 0;
        for _ in 0..count {
            self.range >>= 1;
            let mut bit = 0;
            if self.code >= self.range {
                self.code -= self.range;
                bit = 1;
            }
            if self.code == self.range {
                return Err(Corrupt);
            }
            self.normalize()?;
            value = (value << 1) | bit;
        }
        Ok(value)
    }

    fn tree(&mut self, probs: &mut [u16], base: usize, bits: u32) -> Result<usize, Corrupt> {
        let mut m = 1;
        for _ in 0..bits {
            m = (m << 1) + self.bit(probs, base + m)?;
        }
        Ok(m - (1 << bits))
    }

    fn reverse(&mut self, probs: &mut [u16], base: usize, bits: u32) -> Result<u32, Corrupt> {
        let mut m = 1;
        let mut symbol = 0;
        for i in 0..bits {
            let bit = self.bit(probs, base + m)?;
            m = (m << 1) + bit;
            symbol |= (bit as u32) << i;
        }
        Ok(symbol)
    }
}

struct Lengths {
    choice: [u16; 2],
    low: Vec<u16>,
    mid: Vec<u16>,
    high: Vec<u16>,
}

impl Lengths {
    fn new() -> Self {
        Self {
            choice: [PROB_INIT; 2],
            low: vec![PROB_INIT; POS_STATES_MAX << 3],
            mid: vec![PROB_INIT; POS_STATES_MAX << 3],
            high: vec![PROB_INIT; 1 << 8],
        }
    }

    fn decode(&mut self, rc: &mut Ranges, pos_state: usize) -> Result<usize, Corrupt> {
        if rc.bit(&mut self.choice, 0)? == 0 {
            return rc.tree(&mut self.low, pos_state << 3, 3);
        }
        if rc.bit(&mut self.choice, 1)? == 0 {
            return Ok(8 + rc.tree(&mut self.mid, pos_state << 3, 3)?);
        }
        Ok(16 + rc.tree(&mut self.high, 0, 8)?)
    }
}

fn after_literal(state: usize) -> usize {
    match state {
        0..4 => 0,
        4..10 => state - 3,
        _ => state - 6,
    }
}

pub(crate) fn alone(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    decode(input, limit).ok()
}

fn decode(input: &[u8], limit: usize) -> Result<Vec<u8>, Corrupt> {
    if input.len() < HEADER_BYTES {
        return Err(Corrupt);
    }
    let mut properties = usize::from(input[0]);
    if properties >= 9 * 5 * 5 {
        return Err(Corrupt);
    }
    let lc = properties % 9;
    properties /= 9;
    let lp = properties % 5;
    let pb = properties / 5;
    let dictionary =
        u32::from_le_bytes(input[1..5].try_into().map_err(|_| Corrupt)?).max(MIN_DICTIONARY);
    let size = u64::from_le_bytes(input[5..13].try_into().map_err(|_| Corrupt)?);
    let known = size != u64::MAX;
    let cap = if known {
        usize::try_from(size)
            .ok()
            .filter(|&size| size <= limit)
            .ok_or(Corrupt)?
    } else {
        limit
    };
    let mut out: Vec<u8> = Vec::with_capacity(cap.min(1 << 20));
    let mut rc = Ranges::new(input, HEADER_BYTES)?;
    let mut literals = vec![PROB_INIT; 0x300 << (lc + lp)];
    let mut is_match = vec![PROB_INIT; STATES << 4];
    let mut is_rep = vec![PROB_INIT; STATES];
    let mut is_rep_g0 = vec![PROB_INIT; STATES];
    let mut is_rep_g1 = vec![PROB_INIT; STATES];
    let mut is_rep_g2 = vec![PROB_INIT; STATES];
    let mut is_rep0_long = vec![PROB_INIT; STATES << 4];
    let mut pos_slots = vec![PROB_INIT; LEN_TO_POS_STATES << 6];
    let mut pos_probs = vec![PROB_INIT; 1 + FULL_DISTANCES - END_POS_MODEL as usize];
    let mut align = vec![PROB_INIT; 1 << ALIGN_BITS];
    let mut lengths = Lengths::new();
    let mut rep_lengths = Lengths::new();
    let pos_mask = (1 << pb) - 1;
    let lit_mask = (1 << lp) - 1;
    let mut state = 0;
    let (mut rep0, mut rep1, mut rep2, mut rep3) = (0u32, 0u32, 0u32, 0u32);
    let put = |out: &mut Vec<u8>, byte: u8| {
        if out.len() >= cap {
            return Err(Corrupt);
        }
        out.push(byte);
        Ok(())
    };
    let back = |out: &Vec<u8>, distance: u32| out[out.len() - distance as usize];
    let mut marker = false;
    loop {
        if known && out.len() == cap && rc.code == 0 {
            break;
        }
        let pos_state = out.len() & pos_mask;
        if rc.bit(&mut is_match, (state << 4) + pos_state)? == 0 {
            let previous = out.last().copied().map_or(0, usize::from);
            let base = 0x300 * (((out.len() & lit_mask) << lc) + (previous >> (8 - lc)));
            let mut symbol = 1;
            if state >= 7 {
                let mut match_byte = usize::from(back(&out, rep0 + 1));
                loop {
                    let match_bit = (match_byte >> 7) & 1;
                    match_byte = (match_byte << 1) & 0xff;
                    let bit = rc.bit(&mut literals, base + ((1 + match_bit) << 8) + symbol)?;
                    symbol = (symbol << 1) | bit;
                    if match_bit != bit || symbol >= 0x100 {
                        break;
                    }
                }
            }
            while symbol < 0x100 {
                symbol = (symbol << 1) | rc.bit(&mut literals, base + symbol)?;
            }
            put(&mut out, (symbol - 0x100) as u8)?;
            state = after_literal(state);
            continue;
        }
        let length;
        if rc.bit(&mut is_rep, state)? != 0 {
            if out.is_empty() {
                return Err(Corrupt);
            }
            if rc.bit(&mut is_rep_g0, state)? == 0 {
                if rc.bit(&mut is_rep0_long, (state << 4) + pos_state)? == 0 {
                    state = if state < 7 { 9 } else { 11 };
                    let byte = back(&out, rep0 + 1);
                    put(&mut out, byte)?;
                    continue;
                }
            } else {
                let distance;
                if rc.bit(&mut is_rep_g1, state)? == 0 {
                    distance = rep1;
                } else {
                    if rc.bit(&mut is_rep_g2, state)? == 0 {
                        distance = rep2;
                    } else {
                        distance = rep3;
                        rep3 = rep2;
                    }
                    rep2 = rep1;
                }
                rep1 = rep0;
                rep0 = distance;
            }
            length = rep_lengths.decode(&mut rc, pos_state)?;
            state = if state < 7 { 8 } else { 11 };
        } else {
            rep3 = rep2;
            rep2 = rep1;
            rep1 = rep0;
            length = lengths.decode(&mut rc, pos_state)?;
            state = if state < 7 { 7 } else { 10 };
            let slot = rc.tree(&mut pos_slots, length.min(LEN_TO_POS_STATES - 1) << 6, 6)? as u32;
            if slot < 4 {
                rep0 = slot;
            } else {
                let bits = (slot >> 1) - 1;
                rep0 = (2 | (slot & 1)) << bits;
                if slot < END_POS_MODEL {
                    rep0 += rc.reverse(&mut pos_probs, (rep0 - slot) as usize, bits)?;
                } else {
                    rep0 += rc.direct(bits - ALIGN_BITS)? << ALIGN_BITS;
                    rep0 += rc.reverse(&mut align, 0, ALIGN_BITS)?;
                }
            }
            if rep0 == END_MARKER {
                marker = true;
                break;
            }
            if rep0 >= dictionary || rep0 as usize >= out.len() {
                return Err(Corrupt);
            }
        }
        for _ in 0..length + MATCH_MIN {
            let byte = back(&out, rep0 + 1);
            put(&mut out, byte)?;
        }
    }
    if rc.code != 0 || rc.pos != input.len() || (known && out.len() != cap) || (!known && !marker) {
        return Err(Corrupt);
    }
    Ok(out)
}
