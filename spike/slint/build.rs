fn main() {
    // 기본 스타일(fluent)로 컴파일한다.
    slint_build::compile("ui/main.slint").expect("slint 컴파일 실패");
}
