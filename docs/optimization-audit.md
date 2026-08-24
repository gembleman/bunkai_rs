# `bunkai_rs` 최적화 감사 보고서

작성일: 2026-08-24  
검토 기준: `0a97e5b` (`master`)  
검토 범위: 공개 API, 핵심 스캐너와 규칙 파이프라인, 자료구조/할당, 빌드 설정,
테스트·명세 검증, 벤치마크와 내장 프로파일러

## 결론

`bunkai_rs`는 이미 일반적인 Rust 포팅 수준을 훨씬 넘어선 고도 최적화 상태다.
특히 주 사용 API인 `split_sentences()`는 UTF-8 바이트 단일 패스, 8바이트 경계
레코드, CJK/Kana SWAR 건너뛰기, 이모지 plane-1 직접 인덱스, 통합 예외 필터를
사용한다. 현재 제공된 혼합 일본어 벤치마크에서 7 KB를 약 16.0 us, 56 KB를 약
130 us에 처리한다.

그러나 **“모든 최적화를 다 했다”는 결론은 아니다.** 실제로 세 줄짜리 안전한
ASCII 빠른 거절을 임시 적용했을 때 7 KB 벤치마크가 `split_sentences()`에서
8.82%, `segment()`에서 7.29% 개선됐다. 더 중요하게는, `segment()`의 중간
레이어를 제자리에서 변경하는 기존 최적화가 공개 타입과 문서가 약속하는 “모든
중간 pipeline layer”를 보존하지 않는다. 따라서 현재 상태는 **매우 빠르지만,
최적화가 완료됐다고 선언할 수 없고 한 최적화는 API 의미를 훼손했다**고 평가한다.

## 검증 결과

### 정확성·정적 검사

- `cargo test --all-targets`: 전체 통과
  - 라이브러리 단위 테스트 9개
  - 독립 명세 모델 테스트 6개
  - Python 참조 테스트 14개
  - differential/golden 회귀 테스트 각 1개
- `cargo run --release --bin spec_model`: 전체 통과
  - 생성 11,550건, golden 3,388건, differential 485건
- `cargo clippy --all-targets --all-features -- -D warnings`: 경고 없음

이 검증은 최종 경계의 동등성에는 강하다. 반면 `tests/golden_regression.rs`와 명세
모델은 주로 `final_boundaries`를 비교하며, `segment().layers`의 전체 단계 수와
각 단계 snapshot을 검증하지 않는다. 아래 P0 문제가 테스트를 통과하는 이유다.

### 현재 성능 기준선

환경은 Rust 1.98.0/LLVM 22.1.8, x86-64, AMD Ryzen 7 H 255, release LTO,
`codegen-units=1`이다. Criterion 50 samples 결과의 추정 구간은 다음과 같다.

| 입력 | `segment()` | `split_sentences()` |
| --- | ---: | ---: |
| 61 B | 320.51–321.71 ns | 66.920–66.987 ns |
| 7,005 B | 21.418–21.513 us | 15.986–16.032 us |
| 56,040 B | 167.99–168.29 us | 130.11–130.29 us |

내장 `prof` 기능으로 747,800 B 혼합 corpus를 400회 처리했을 때 `segment()`의
누적 단계 비중은 `scan_all` 59.6%, `into_output` 11.6%, `LinebreakForce`
7.7%, `i.unify` 7.4%, `BasicRule` 4.5%, materialize 3.1%였다. 계측 자체의
오버헤드가 있으므로 절대 시간보다 병목의 방향을 보는 자료다. 하드웨어 카운터는
호스트의 `perf_event_paranoid=4` 제한 때문에 수집하지 못했다.

## 이미 잘 적용된 최적화

- `split_sentences()`와 `sentence_byte_boundaries()`가 레이어 생성 없이 바이트
  경계를 직접 만든다.
- `Vec<char>`와 char-to-byte map을 경계 전용 경로에서 제거했다.
- CJK/Kana의 안전한 3바이트 run을 SWAR로 일괄 건너뛴다.
- plane-1 이모지는 약 5.6 KB 직접 인덱스 표를 사용하고, CJK emoji-free hole과
  Kana 범위를 빠르게 거절한다.
- 네 후보 탐지기와 줄바꿈 탐지를 한 스캔으로 합쳤다.
- 정규식과 외부 런타임 의존성이 없고, whitespace 의미를 직접 고정했다.
- 내부 `SpanRecord`는 32 B, 경계 전용 `Bound`는 8 B이며 문자열을 입력에서
  빌린다.
- 경계 전용 경로의 세 예외 규칙은 하나의 `retain`으로 합쳐졌다.
- 줄바꿈 없는 빠른 경로, 정렬 후 dedup, LTO와 단일 codegen unit이 적용됐다.
- 최적화 전 구현, 독립 명세 모델, Python 파생 corpus와의 다층 회귀 검증이 있다.

## 발견 사항과 우선순위

### P0 — `segment()`가 모든 중간 Layer를 보존하지 않는다

`Segmentation`의 공개 문서는 `layers`가 “every intermediate pipeline stage”를
기록한다고 설명한다. 하지만 `apply_indirect_quote`, `apply_dot_exception`,
`apply_number_exception`, `apply_linebreak_force`는 새 레이어를 추가하지 않고
현재 마지막 레이어를 `retain`/덮어쓰기한 뒤 이름만 변경한다. 결과적으로 독립된
`BasicRule`, `IndirectQuoteExceptionAnnotator`, `DotExceptionAnnotator`,
`NumberExceptionAnnotator` snapshot이 사라진다.

이는 단순한 추가 최적화 후보가 아니라 의미 보존 실패다. 우선 다음 중 하나를
명시적으로 선택해야 한다.

1. `segment()`는 각 단계 snapshot을 다시 보존하고, 현재의 경계 전용 최적화는
   `split_sentences()`/`sentence_byte_boundaries()`에만 둔다. 공개 계약과 가장
   잘 맞는 권장안이다.
2. 레이어 이력을 제공하지 않는 별도 `segment_final()` 같은 API를 추가한다.
3. 정말 마지막 누적 레이어만 제공할 의도라면 공개 문서와 타입 이름을 바꾸고
   breaking change로 취급한다.

회귀 테스트에는 레이어 이름 배열, 레이어 수, 각 단계 span snapshot을 최적화 전
구현과 비교하는 검사를 추가해야 한다. 최종 경계만 비교해서는 이 문제를 잡지
못한다.

### P1 — ASCII가 불필요한 이모지 이진 탐색을 수행한다 (측정 완료)

`EMOJI_DATA`의 첫 codepoint는 U+00A9지만 `emoji_lookup()`은 U+0000–U+00A8에
대한 빠른 거절이 없다. 따라서 일반 영문자와 숫자가 모두 약 170개 low block을
이진 탐색한다. 혼합 문서의 Latin, `No.1`, 소수, 이메일에 반복되는 비용이다.

감사 중 다음 조건을 임시 적용하고 테스트·벤치마크한 뒤 소스는 원상복구했다.

```rust
if codepoint < 0xA9 {
    return None;
}
```

Criterion이 보고한 7,005 B 변화는 다음과 같다.

| API | 기준선 중앙값 | 임시 패치 중앙값 | Criterion 변화 추정 |
| --- | ---: | ---: | ---: |
| `segment()` | 21.463 us | 20.029 us | -7.29% |
| `split_sentences()` | 16.008 us | 14.658 us | -8.82% |

패치 상태에서도 release 라이브러리 테스트 9개가 모두 통과했다. 데이터 테이블의
최솟값으로 안전성을 compile-time assert하거나 생성 코드에서 상수를 만들면 매우
낮은 위험으로 반영할 수 있다. 이 결과 하나만으로도 “남은 최적화가 없다”는 주장은
반증된다.

### P1 — 이모지 lookup 반환값과 high table을 더 작게 만들 수 있다

현재 `emoji_lookup()`은 `(Option<&str>, bool)`을 반환하지만 모든 hot-path 호출자는
category 문자열을 버리고 membership과 target 여부만 사용한다. plane-1에서도
`u16 index -> EMOJI_DATA category + EMOJI_TARGET`의 추가 load가 발생한다.

lookup 결과를 `Absent / NonTarget / Target` 3상태 `u8`로 바꾸고 high table에 그
값을 직접 저장하면 표를 약 5.6 KB에서 2.8 KB로 줄이고 간접 load를 없앨 수 있다.
공개 `EMOJI_DATA`는 호환성을 위해 그대로 유지할 수 있다. 이 변경은 ASCII 빠른
거절과 함께/별도로 benchmark해야 한다.

### P2 — full-layer 줄바꿈 경로만 아직 `HashMap`을 쓴다

경계 전용 `apply_linebreak_force_bounds()`는 정렬된 `linebreaks`에 이진 탐색과
consumed bitmap을 사용한다. 반면 `segment()` 경로의 `apply_linebreak_force()`는
같은 정렬 정보를 두고도 `HashMap<usize, (usize, usize)>`를 구성한다. 줄바꿈이
없으면 loop 자체를 건너뛰고 이름만 확정할 수 있고, 줄바꿈이 있으면 경계 경로의
검증된 전략을 공유할 수 있다.

단, P0에 따라 각 단계 snapshot을 복구한다면 이 함수는 새 레이어를 만들어야
하므로, 먼저 API 의미를 확정한 뒤 자료구조를 최적화해야 한다.

### P2 — owned `current` span을 불필요하게 clone한다

`filter_previous_rule_same_span()`은 `current.into_iter()`로 소유권을 이미 받지만,
peek한 현재 항목을 `out.push(c.clone())`한 뒤 원본을 버린다. 비교용 키를 먼저
복사하고 `ci.next().unwrap()` 값을 move하면 fresh-rule `SpanRecord`의 32 B 복사를
피할 수 있다. 이전 레이어 snapshot을 유지하기 위한 `previous` clone은 공개
레이어 이력이 필요하다면 불가피하다.

### P2 — `split_sentences()`의 중간 경계 Vec 할당

현재 fast path는 `Vec<Bound>`를 만든 뒤 `Vec<usize>` 경계로 복사하고, 다시
`Vec<&str>` 결과를 만든다. 내부 전용 함수가 정규화된 `Bound`를 직접 소비해
문자열 slice를 만들게 하면 `split_sentences()` 호출당 한 번의 Vec 할당과 복사를
줄일 여지가 있다. 공개 `sentence_byte_boundaries()`는 현재 반환 타입을 유지하면
된다. 짧은 입력에서는 스캔보다 고정 할당 비용의 비중이 크므로 별도 benchmark
가치가 있다.

### P3 — ASCII boring-run과 추가 CJK 영역의 bulk skip

현재 bulk skip은 주로 U+3040–U+30FF와 U+4000–U+9FFF를 대상으로 한다. 긴 ASCII
단어/숫자 run, CJK Extension A의 일부(U+3400–U+3FFF), plane-2 ideograph는 문자별
decode와 dispatch를 거친다. ASCII 빠른 거절을 먼저 적용한 뒤에도 ASCII-heavy
workload가 중요하다면, `.!?(`, whitespace 등 trigger를 제외한 boring byte run을
word-at-a-time으로 넘기는 실험을 할 수 있다. 복잡성과 검증 부담이 커지므로 P1/P2
이후에 실제 corpus로 판단해야 한다.

### P3 — 4 GiB 이상 입력의 `u32` 축소를 명시해야 한다

`Bound`는 byte offset을, `SpanRecord`는 char offset을 `u32`로 저장한다. 여러
지점에서 `as u32` cast가 release에서도 무조건 수행되어 매우 큰 입력에서는
조용히 wrap될 수 있다. 현실성은 낮지만 공개 API가 크기를 제한하지 않는다.
최적화의 전제조건으로 입력 길이를 검사해 명확히 panic/error 처리하거나, 큰 입력용
`usize` fallback을 제공해야 한다. 성능 최적화가 안전성 전제를 암묵적으로 만들면
안 된다.

## 벤치마크의 사각지대

현재 Criterion bench는 좋은 시작점이지만 같은 20문장을 반복해 short/medium/long
크기만 바꾼다. 이 결과만으로 전 workload의 최적화를 선언할 수 없다. 최소한 다음
축을 독립적으로 추가해야 한다.

- 순수 Kana/Kanji, Latin/ASCII-heavy, emoji-heavy, kaomoji-heavy
- 경계가 없는 긴 문서와 구두점 밀도가 높은 문서
- 줄바꿈 없음/희소/매우 많음
- 0 B, 1–16 B, 일반 채팅 크기, 수 MB batch
- `segment()`의 span 수가 적은/많은 경우
- latency뿐 아니라 bytes/s, allocations/call, peak resident memory
- x86-64 외 AArch64와 generic CPU target

최적화별로 `cargo test --release --all-targets`, 독립 spec model, layer snapshot
회귀(P0 해결 후), Criterion 통계, 가능하면 cycles/byte·instructions/byte를 통과
조건으로 삼아야 한다.

## 권장 실행 순서

1. P0 레이어 계약을 테스트로 고정하고 `segment()` 의미를 복구한다.
2. U+00A9 미만 ASCII 빠른 거절을 반영한다. 이미 약 7–9% 개선이 측정됐다.
3. 이모지 3상태 lookup/table을 실험하고 ASCII 패치 위에서 추가 이득을 측정한다.
4. full-layer 줄바꿈 `HashMap`과 owned-current clone을 제거한다.
5. `split_sentences()`가 중간 `Vec<usize>`를 만들지 않는 내부 경로를 실험한다.
6. workload matrix와 allocation/counter 측정을 갖춘 뒤에만 bulk skip/SIMD 범위를
   넓힌다.

요약하면, 현재 구현은 최적화 수준이 높고 최종 경계 정확성 검증도 강하다. 하지만
측정 가능한 저위험 개선이 남아 있고, 레이어 보존 계약은 성능보다 먼저 바로잡아야
한다. 따라서 현시점의 적절한 판정은 **“매우 잘 최적화됨, 완료 아님”**이다.
