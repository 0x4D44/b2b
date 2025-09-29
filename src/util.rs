use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStderr, ChildStdout},
};

pub fn spawn_reader_thread<R: Send + 'static + std::io::Read>(
    reader: R,
    tag: String,
    mut cb: impl FnMut(String, &str) + Send + 'static,
) {
    std::thread::spawn(move || {
        let br = BufReader::new(reader);
        for line in br.lines() {
            match line {
                Ok(l) => cb(tag.clone(), l.trim_end()),
                Err(_) => break,
            }
        }
    });
}

pub fn child_pipes(child: &mut Child) -> (Option<ChildStdout>, Option<ChildStderr>) {
    (child.stdout.take(), child.stderr.take())
}
