# `split_sentences()` 추가 최적화 로드맵

작성일: 2026-08-24  
범위: `split_sentences()`와 `sentence_byte_boundaries()`의 UTF-8 byte fast path  
제외: 전체 pipeline snapshot을 생성하는 `segment()`

## 현재 기준

이모지 lookup 최적화 후 7,005 B 혼합 일본어 입력의 `split_sentences()`는
13.949–13.963 us, 약 478.7 MiB/s를 기록했다. 이후 아래 P1 작업까지 적용한
최신 결과는 13.460–13.497 us, 약 495.5 MiB/s다. 감사 당시 기준인
15.986–16.032 us보다 약 15.8% 빠르다.

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

## 2026-08-24 실행 결과

### 완료 — ASCII boring-run bulk skip

긴 영문·숫자 구간에서 다음 trigger를 포함하지 않는 연속 구간을 8-byte SWAR로
건너뛰도록 구현했다.

- `.` / `!` / `?` / `(`
- ASCII whitespace
- high bit가 설정된 UTF-8 byte

첫 byte가 boring ASCII일 때만 packed 검사를 시작해 혼합 문서의 구두점에서 불필요한
setup을 피한다. high bit 또는 control/space가 있는 word는 scalar path로 넘기며, skip
뒤의 `prev` 상태는 boring sentinel로 설정한다.

측정 결과:

- ASCII-heavy 8,704 B: 42.396 us → 18.831 us, 약 55.6% 개선
- no-boundary 11,680 B: 52.296 us → 19.427 us, 약 62.9% 개선
- 혼합 7,005 B: 13.956 us → 13.481 us, 약 3.4% 추가 개선
- target-specific intrinsic 없이 안전한 `u64` load와 정수 연산만 사용
- 전체 golden/differential/spec model 동등성 통과

### 완료 — 단일 문장용 allocation-free 내부 결과

scanner가 예외 적용 후에도 유효 경계와 줄바꿈을 찾지 못하면 내부 `WholeText` 결과를
반환하도록 구현했다. `split_sentences()`는 중간 boundary vector 없이 바로
`vec![text]`를 만들며, 빈 문자열은 allocation 없는 `Vec::new()`를 반환한다. 공개
`sentence_byte_boundaries()`의 반환형과 결과는 그대로 유지된다.

1-byte 입력은 약 31.7 ns에서 8.02 ns로 약 74.7% 개선됐다. no-boundary 결과에는
위 ASCII bulk skip 효과도 함께 포함된다. 후보가 있는 입력은 기존 `Vec<usize>` 정렬
경로를 그대로 사용한다.

## 남은 후보

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

## 다음 실행 순서

1. allocations/call 계측을 추가해 단일 문장 경로의 allocation 감소를 수치로 고정한다.
2. low emoji bitset은 emoji/symbol-heavy baseline을 먼저 추가한 뒤 실험한다.
3. AArch64에서도 ASCII SWAR의 개선과 혼합 workload 무회귀를 확인한다.
4. 각 변경은 독립 commit과 Criterion baseline으로 유지해 회귀 시 즉시 제거한다.

두 P1 후보는 반영을 완료했다. 다음 구현 후보는 low emoji bitset이지만, 일반 일본어
입력보다 특수 symbol workload에 치우친 최적화이므로 전용 baseline 없이 적용하지
않는다. `Vec<Bound>` 직접 소비 방식은 실제 측정상 회귀했으므로 계속 제외한다.
