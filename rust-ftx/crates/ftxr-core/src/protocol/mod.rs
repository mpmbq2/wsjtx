// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ftxr-modem Contributors
// Based on WSJT-X algorithms (Copyright (C) 2001-2026 WSJT Development Group).

//! 77-bit WSJT-X message codec, callsign hash table, CRC-14, LDPC(174,91), and FT4 `rvec`.

pub mod crc14;
pub mod hashtable;
pub mod ldpc;
pub mod pack77;
pub mod rvec;
pub mod types;
pub mod unpack77;

#[cfg(test)]
mod tests {
    use super::hashtable::{ihashcall, CallsignHashTable};
    use super::pack77::{chkcall, fmtmsg, pack28, pack77, split77, stdcall, MAXGRID4, NTOKENS};
    use super::types::{MessageType, PackHint, UnpackContext, UnpackMode};
    use super::unpack77::{callok, unpack28, unpack77};

    #[test]
    fn n28_boundary_values_and_predicates() {
        let ht = CallsignHashTable::new();

        // Special token boundaries
        assert_eq!(unpack28(0, &ht), ("DE".to_string(), true));
        assert_eq!(unpack28(1, &ht), ("QRZ".to_string(), true));
        assert_eq!(unpack28(2, &ht), ("CQ".to_string(), true));
        assert_eq!(unpack28(3, &ht), ("CQ_000".to_string(), true));
        assert_eq!(unpack28(1002, &ht), ("CQ_999".to_string(), true));
        assert_eq!(unpack28(1003, &ht), ("CQ_".to_string(), true));
        assert_eq!(unpack28(532_443, &ht), ("CQ_ZZZZ".to_string(), true));
        // Gap 532_444..=2_063_591 returns "<...>" with success=true
        assert_eq!(unpack28(532_444, &ht), ("<...>".to_string(), true));
        assert_eq!(unpack28(2_063_591, &ht), ("<...>".to_string(), true));
        // 22-bit hash range 2_063_592..=6_257_895
        assert_eq!(unpack28(2_063_592, &ht), ("<...>".to_string(), true));
        assert_eq!(unpack28(6_257_895, &ht), ("<...>".to_string(), true));
        // Standard callsign boundaries: 6_257_896 = " 00   " -> fails callok -> ("QU1RK", false)
        assert_eq!(unpack28(6_257_896, &ht), ("QU1RK".to_string(), false));
        // Max 28-bit value 268_435_455 = "ZZ9ZZZ"
        assert_eq!(unpack28(268_435_455, &ht), ("ZZ9ZZZ".to_string(), true));

        // Round-trip pack28 -> unpack28 on standard & special calls
        for tok in [
            "DE", "QRZ", "CQ", "CQ_000", "CQ_999", "CQ_A", "CQ_DX", "CQ_ZZZZ", "K1A", "K1ABC",
            "KA1BC", "W9XYZ", "PJ4A", "G4ABC", "ZZ9ZZZ",
        ] {
            let n28 = pack28(tok, Some(&ht), None).unwrap();
            let (out, ok) = unpack28(n28, &ht);
            assert!(ok, "unpack28 failed for {tok}");
            assert_eq!(out, tok);
        }

        // Nonstandard call falls into hash range and resolves from ht
        let n28_3da = pack28("3DA0XYZ", Some(&ht), None).unwrap();
        assert!((NTOKENS..NTOKENS + 4_194_304).contains(&n28_3da));
        assert_eq!(unpack28(n28_3da, &ht), ("<3DA0XYZ>".to_string(), true));

        // chkcall vs callok vs stdcall edge cases
        assert_eq!(chkcall("QU1RK").as_deref(), Some("QU1RK"));
        assert!(!callok("QU1RK"));
        assert_eq!(chkcall("FN42"), None);
        assert!(!callok("FN42"));
        assert_eq!(chkcall("KA1"), None); // chkcall requires 1-3 letter suffix
        assert!(callok("KA1")); // callok allows 0-letter suffix
        assert!(stdcall("K1ABC"));
        assert!(!stdcall("PJ4/K1ABC"));
        assert_eq!(chkcall("PJ4/K1ABC").as_deref(), Some("K1ABC"));
        assert_eq!(chkcall("K1ABC/R").as_deref(), Some("K1ABC"));
        assert_eq!(chkcall("K1ABC/P").as_deref(), Some("K1ABC"));
        assert_eq!(chkcall("3DA0XYZ"), None);
    }

    #[test]
    fn split77_and_fmtmsg_rules() {
        assert_eq!(fmtmsg("  cq   k1abc   fn42  "), " CQ K1ABC FN42");
        let (collapsed, words) = split77("  cq   dx   k1abc   fn42  ");
        assert_eq!(collapsed, "CQ DX K1ABC FN42");
        assert_eq!(words, vec!["CQ_DX", "K1ABC", "FN42"]);

        // Does not merge when 3rd word is not a valid callsign
        let (_, words2) = split77("CQ K1ABC FN42");
        assert_eq!(words2, vec!["CQ", "K1ABC", "FN42"]);
    }

    #[test]
    fn adversarial_codec_quirks() {
        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();

        // RR73 encodes as igrid4 = 32373, not 32403
        let rr73 = pack77("K1ABC W9XYZ RR73", None, &ctx, &ht).unwrap();
        assert_eq!(rr73.i3, 1);
        assert_eq!(rr73.msgsent, "K1ABC W9XYZ RR73");
        let mut igrid4 = 0u32;
        for &b in &rr73.bits[59..74] {
            igrid4 = (igrid4 << 1) | (b as u32);
        }
        assert_eq!(igrid4, 32373);
        // Legacy 32403 also unpacks as RR73
        let mut legacy_bits = rr73.bits;
        let legacy_val = MAXGRID4 + 3;
        for i in 0..15 {
            legacy_bits[59 + i] = ((legacy_val >> (14 - i)) & 1) as u8;
        }
        assert_eq!(
            unpack77(&legacy_bits, UnpackMode::Rx, &ctx, &ht)
                .unwrap()
                .text,
            "K1ABC W9XYZ RR73"
        );

        // Type 4 CQ <nonstd> writes ihashcall(w[1], 12) into n12
        let cq_ns = pack77("CQ KH1/KH7Z", None, &ctx, &ht).unwrap();
        assert_eq!(cq_ns.i3, 4);
        assert_eq!(cq_ns.msgsent, "CQ KH1/KH7Z");
        let mut n12 = 0u32;
        for &b in &cq_ns.bits[..12] {
            n12 = (n12 << 1) | (b as u32);
        }
        assert_eq!(n12, ihashcall("KH1/KH7Z", 12));

        // WSPR 0.6 Type 1 & Type 2 & Type 3
        let w1 = pack77("K1ABC FN42 30", None, &ctx, &ht).unwrap();
        assert_eq!((w1.i3, w1.n3), (0, 6));
        assert_eq!(w1.msgsent, "K1ABC FN42 30");

        let w1_clamp = pack77("K1ABC FN42 73", None, &ctx, &ht).unwrap();
        assert_eq!((w1_clamp.i3, w1_clamp.n3), (0, 6));
        assert_eq!(w1_clamp.msgsent, "K1ABC FN42 60");

        let w3 = pack77("<PJ4/K1ABC> FN42AB", Some(PackHint::Wspr), &ctx, &ht).unwrap();
        assert_eq!((w3.i3, w3.n3), (0, 6));
        assert_eq!(w3.msgsent, "<PJ4/K1ABC> FN42AB");

        // Free text fallbacks
        let ft1 = pack77("CQ ABCD", None, &ctx, &ht).unwrap();
        assert_eq!((ft1.i3, ft1.n3), (0, 0));
        assert_eq!(ft1.msgsent, "CQ ABCD");

        let ft_tel_overflow = pack77("800000000000000000", None, &ctx, &ht).unwrap();
        assert_eq!((ft_tel_overflow.i3, ft_tel_overflow.n3), (0, 0));

        // $DX substitution
        let dx_ctx = UnpackContext {
            dx_base: Some("K1ABC".to_string()),
            ..Default::default()
        };
        let dx_msg = pack77("$DX W9XYZ -10", None, &dx_ctx, &ht).unwrap();
        assert_eq!(dx_msg.i3, 1);
        assert_eq!(dx_msg.msgsent, "K1ABC W9XYZ -10");
    }

    #[test]
    fn ft8_testmsg_49_vectors_roundtrip() {
        let msgs: [(&str, u8, u8); 49] = [
            ("TNX BOB 73 GL", 0, 0),
            ("K1ABC RR73; W9XYZ <KH1/KH7Z> -08", 0, 1),
            ("K1ABC W9XYZ 6A WI", 0, 3),
            ("W9XYZ K1ABC R 17B EMA", 0, 4),
            ("123456789ABCDEF012", 0, 5),
            ("CQ K1ABC FN42", 1, 0),
            ("K1ABC W9XYZ EN37", 1, 0),
            ("W9XYZ K1ABC -11", 1, 0),
            ("K1ABC W9XYZ R-09", 1, 0),
            ("W9XYZ K1ABC RRR", 1, 0),
            ("K1ABC W9XYZ 73", 1, 0),
            ("K1ABC W9XYZ RR73", 1, 0),
            ("CQ FD K1ABC FN42", 1, 0),
            ("CQ TEST K1ABC/R FN42", 1, 0),
            ("K1ABC/R W9XYZ EN37", 1, 0),
            ("W9XYZ K1ABC/R R FN42", 1, 0),
            ("K1ABC/R W9XYZ RR73", 1, 0),
            ("CQ TEST K1ABC FN42", 1, 0),
            ("W9XYZ <PJ4/K1ABC> -11", 1, 0),
            ("<PJ4/K1ABC> W9XYZ R-09", 1, 0),
            ("CQ W9XYZ EN37", 1, 0),
            ("<YW18FIFA> W9XYZ -11", 1, 0),
            ("W9XYZ <YW18FIFA> R-09", 1, 0),
            ("<YW18FIFA> KA1ABC", 1, 0),
            ("KA1ABC <YW18FIFA> -11", 1, 0),
            ("<YW18FIFA> KA1ABC R-17", 1, 0),
            ("<YW18FIFA> KA1ABC 73", 1, 0),
            ("CQ G4ABC/P IO91", 2, 0),
            ("CQ TEST G4ABC/P IO91", 2, 0),
            ("<PA9XYZ> <G4ABC/P> 570123 IO91NP", 5, 0),
            ("<G4ABC/P> <PA9XYZ> R 580071 JO22DB", 5, 0),
            ("G4ABC/P PA9XYZ JO22", 2, 0),
            ("PA9XYZ G4ABC/P RR73", 2, 0),
            ("K1ABC W9XYZ 579 WI", 3, 0),
            ("W9XYZ K1ABC R 589 MA", 3, 0),
            ("K1ABC KA0DEF 559 MO", 3, 0),
            ("TU; KA0DEF K1ABC R 569 MA", 3, 0),
            ("KA1ABC G3AAA 529 0013", 3, 0),
            ("TU; G3AAA K1ABC R 559 MA", 3, 0),
            ("CQ KH1/KH7Z", 4, 0),
            ("CQ PJ4/K1ABC", 4, 0),
            ("PJ4/K1ABC <W9XYZ>", 4, 0),
            ("<W9XYZ> PJ4/K1ABC RRR", 4, 0),
            ("PJ4/K1ABC <W9XYZ> 73", 4, 0),
            ("<W9XYZ> YW18FIFA", 4, 0),
            ("YW18FIFA <W9XYZ> RRR", 4, 0),
            ("<W9XYZ> YW18FIFA 73", 4, 0),
            ("CQ YW18FIFA", 4, 0),
            ("<KA1ABC> YW18FIFA RR73", 4, 0),
        ];

        let ht = CallsignHashTable::new();
        let ctx = UnpackContext::default();
        for (msg, exp_i3, exp_n3) in msgs {
            let packed = pack77(msg, None, &ctx, &ht).unwrap();
            assert_eq!(
                (packed.i3, packed.n3),
                (exp_i3, exp_n3),
                "type mismatch for {msg}"
            );
            assert_eq!(packed.msgsent, msg, "roundtrip mismatch for {msg}");
            assert_eq!(
                MessageType::from_i3_n3(exp_i3, exp_n3),
                Some(packed.msg_type)
            );
        }
    }
}
