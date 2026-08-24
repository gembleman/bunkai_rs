# `split_sentences()` 추가 최적화 로드맵

작성일: 2026-08-24  
범위: `split_sentences()`와 `sentence_byte_boundaries()`의 UTF-8 byte fast path  
제외: 전체 pipeline snapshot을 생성하는 `segment()`

## 현재 기준

이번 최적화 후 7,005 B 혼합 일본어 입력의 `split_sentences()`는
13.949–13.963 us, 약 478.7 MiB/s를 기록했다. 감사 당시 기준인
15.986–16.032 us보다 약 12.8% 빠르다.

반영된 핵심 변경은 다음과 같다.

- U+00A9 미만 codepoint를 이모지 table 접근 전에 거절한다.
- 이모지 lookup 결과를 `Absent / NonTarget / Target`의 `u8` 3상태로 표현한다.
- plane-1 직접 lookup table을 약 5.6 KB의 `u16` index table에서 약 2.8 KB의
  `u8` state table로 줄이고 간접 category load를 제거한다.
- `u32` byte offset이 표현할 수 없는 4 GiB 초과 입력은 조용히 wrap하지 않고
  명시적으로 거절한다.

`Vec<Bound>`를 직접 정렬한 뒤 slice 생성까지 소비하여 중간 `Vec<usize>`를 없애는
실험은 같은 입력에서 약 11% 느려졌다. 8-byte payload 자체보다 정렬·dedup 코드 생성과
후속 소비 형태가 불리했던 것으로 보이며, 이 형태는 다시 적용하지 않는다.

## 다음 후보

### P1 — ASCII boring-run bulk skip

현재 byte scanner는 ASCII에 대한 이모지 binary search는 피하지만, 긴 영문·숫자 구간을
여전히 byte마다 decode하고 `match`로 dispatch한다. ASCII-heavy 입력에서 다음 trigger를
포함하지 않는 연속 구간을 word-at-a-time으로 건너뛸 수 있다.

- `.` / `!` / `?` / `(`
- ASCII whitespace
- high bit가 설정된 UTF-8 byte

8-byte word에서 위 byte들의 존재 여부를 SWAR zero-byte 검출로 검사하고, 안전한 word만
한 번에 넘기는 방식이 우선 실험 대상이다. skip 뒤의 `prev` 상태는 boring ASCII
sentinel로 설정해야 한다. `ascii-heavy`, `no-boundary`, `punctuation-dense` workload를
각각 측정해 trigger가 많은 입력에서의 branch overhead까지 확인한다.

통과 조건:

- 전체 golden/differential/spec model 동등성
- `ascii-heavy`와 `no-boundary`에서 통계적으로 유의한 개선
- 혼합 7 KB 및 tiny 입력에서 2%를 넘는 회귀 없음
- x86-64와 AArch64 모두 target-specific intrinsic 없이 동일 의미 유지

### P1 — 단일 문장용 allocation-free 내부 결과

경계 후보가 없는 일반 문자열도 현재는 최종 `[text.len()]`을 담는 boundary `Vec`와
결과 `Vec<&str>`를 각각 만든다. scanner 결과가 “후보와 줄바꿈 없음”임을 알려주면
`split_sentences()`는 boundary vector를 만들지 않고 바로 `vec![text]`를 반환할 수 있다.

빈 문자열, 구두점 없는 채팅, 긴 no-boundary 문서에서 효과가 기대된다. 공개
`sentence_byte_boundaries()`는 기존 반환형과 동작을 유지하고, 내부 전용 경로만 별도로
둔다. 후보가 있는 입력은 현재의 `Vec<usize>` 정렬 경로를 그대로 사용해 앞선 실패
실험을 반복하지 않는다.

통과 조건:

- 0 B, 1–16 B, `no-boundary`의 latency와 allocations/call 개선
- 문장 경계가 있는 medium/long 입력에 측정 가능한 회귀 없음

### P2 — low emoji block의 compact membership 실험

U+00A9–U+3299 영역은 아직 약 170개 항목을 binary search한다. 이 구간에 대해
membership bitset과 target bitset을 사용하면 lookup을 고정된 두 bit load로 바꿀 수
있다. 두 bitset의 합은 약 3.2 KB이므로 기존 table을 완전히 대체하지 않더라도 cache
비용이 작다.

ASCII는 이미 빠르게 거절되므로 일반 일본어 corpus에서의 이득은 제한적일 수 있다.
`emoji-heavy`와 Latin-1/symbol-heavy corpus에서만 유의미하고 혼합 workload에 회귀가
없을 때 반영한다.

### P2 — allocation과 CPU counter 계측

Criterion latency/throughput만으로는 fast path의 개선 원인을 구분하기 어렵다. 다음
측정을 별도 bench 또는 profiler에 추가한다.

- allocations/call과 allocated bytes/call
- cycles/byte, instructions/byte, branch misses
- 0 B, tiny, no-boundary, ASCII-heavy, CJK/Kana, emoji-heavy, newline-dense
- x86-64와 AArch64

특히 allocation-free 단일 문장 경로는 latency뿐 아니라 allocations/call 감소를 필수
통과 조건으로 삼는다.

## 실행 순서

1. ASCII boring-run prototype을 만들고 workload matrix 전체를 비교한다.
2. 단일 문장 내부 fast return을 구현해 tiny/no-boundary allocation을 측정한다.
3. low emoji bitset은 emoji/symbol-heavy 결과가 중요한 workload에서만 실험한다.
4. 각 변경은 독립 commit과 Criterion baseline으로 유지해 회귀 시 즉시 제거한다.

현재 가장 유력한 후보는 ASCII boring-run이며, 가장 낮은 복잡도로 allocation을 줄일
후보는 단일 문장 내부 fast return이다. 반면 `Vec<Bound>` 직접 소비 방식은 실제 측정상
회귀했으므로 제외한다.
