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

## P2 실행 결과

### 실험 완료·미반영 — low emoji block compact membership

U+00A9–U+3299의 상태를 `Absent / NonTarget / Target` 2-bit packed table로 만들어
binary search와 category flag load를 대체했다. 공개 `EMOJI_DATA`는 그대로 유지했다.
paired Criterion 결과는 다음과 같다.

- 혼합 7 KB: 13.530 us → 13.105 us, 3.43% 개선
- low-symbol-heavy: 42.904 us → 34.919 us, 18.65% 개선
- plane-1 emoji/kaomoji: 37.925 us → 38.358 us, 1.44% 회귀

low lookup을 `#[inline(never)]` 함수로 격리해 plane-1 hot code의 크기를 줄이는 변형도
시험했다. 이 경우 low-symbol은 6.66% 개선됐지만 plane-1 emoji/kaomoji가 4.74%
악화됐다. 특정 symbol workload의 이득보다 일반 emoji workload 회귀가 커서 두 변형
모두 되돌렸다. low-symbol 전용 8,544 B Criterion corpus는 이후 구현 변형을 비교할 수
있도록 유지한다.

### 완료 — allocation 계측

`examples/allocation_metrics.rs`에 `System` global allocator를 감싼 독립 계측 도구를
추가했다. warmup 뒤 counter를 reset하고 `split_sentences()` 한 호출의 성공한
alloc/realloc 횟수와 요청 byte 합계를 측정한다. 출력과 formatting은 측정 구간 밖에서
수행한다.

```text
cargo run --release --example allocation_metrics
```

| workload | input | allocations/call | allocated bytes/call |
| --- | ---: | ---: | ---: |
| empty | 0 B | 0 | 0 |
| tiny | 1 B | 1 | 16 |
| no-boundary | 55 B | 1 | 16 |
| ASCII-heavy | 5,760 B | 8 | 5,112 |
| medium | 12,544 B | 19 | 37,936 |

빈 입력은 allocation이 없고, 유효 경계가 없는 입력은 결과 `Vec<&str>` 한 번만
할당한다. 따라서 P1의 내부 boundary allocation 제거가 의도대로 작동함을 수치로
고정했다.

## 남은 검증 후보

### CPU counter와 타 architecture

Criterion latency/throughput만으로는 fast path의 개선 원인을 구분하기 어렵다. 다음
측정은 호스트 권한이나 별도 실행 환경이 필요하다.

- cycles/byte, instructions/byte, branch misses
- x86-64와 AArch64

## 다음 실행 순서

1. AArch64에서 ASCII SWAR의 개선과 혼합 workload 무회귀를 확인한다.
2. `perf_event_paranoid`가 허용되는 호스트에서 CPU counter를 수집한다.
3. low emoji lookup은 plane-1 회귀를 구조적으로 피할 새 설계가 있을 때만 재실험한다.
4. 각 변경은 독립 commit과 Criterion baseline으로 유지해 회귀 시 즉시 제거한다.

두 P1 후보와 allocation 계측은 반영을 완료했다. low emoji bitset과 `Vec<Bound>` 직접
소비 방식은 특정 workload의 이득보다 다른 중요 workload의 회귀가 확인되어 제외한다.
