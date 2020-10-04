use indexed_file_storage::dirtreefile::DirTreeFile;
use std::error::Error;
use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

pub fn main() {
    let mut filetree = DirTreeFile::new(PathBuf::from("./testdata/test.dft"));
    filetree.init().unwrap();
    println!("{:?}", filetree.entries().unwrap());
    filetree.create_entry("test", true);
    filetree.create_entry("test2", false);
    println!("{:?}", filetree.entries().unwrap());
    filetree.cd("test");
    println!("{:?}", filetree.entries().unwrap());
    for i in 0..1000 {
        let path = format!("test-{}", i);
        filetree.create_entry(path.as_str(), true);
        filetree.cd(path.as_str()).unwrap();
        for j in 0..10 {
            filetree.create_entry(format!("{}'s_child-{}", path, j).as_str(), false);
        }
        filetree.cd("..").unwrap();
    }
    filetree.delete_entry("test-4").unwrap();
    filetree.delete_entry("test-23").unwrap();
    filetree.cd("/").unwrap();
    traverse_tree(0, &mut filetree);
}

fn traverse_tree(indent: usize, filetree: &mut DirTreeFile) {
    let indent_string = " ".repeat(indent);
    let entries = filetree.entries().unwrap_or_else(|error| {
        return Vec::default();
    });

    for entry in entries {
        println!("{}{}", indent_string, entry.name);
        if entry.is_dir() {
            filetree.cd(entry.name.as_str()).unwrap();
            traverse_tree(indent + 2, filetree);
            filetree.cd("..").unwrap();
        }
    }
}
