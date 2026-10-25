// 트레이/창 아이콘 리소스(app.rc)를 컴파일해 링크한다. gnu 툴체인에서는 windres 를 쓴다.
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/icon.ico");
    embed_resource::compile("res/app.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
