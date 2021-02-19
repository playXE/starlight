use std::{fs::OpenOptions, io::Write, panic::AssertUnwindSafe};

use starlight::{
    gc::handle::Handle,
    runtime::{arguments::Arguments, value::JsValue},
    vm::{Options, VirtualMachine, VirtualMachineRef},
};

use test262_harness::*;
static mut RUNNING: Option<String> = None;
fn main() {
    let _ = std::fs::remove_file("test262_result");
    std::panic::set_hook(Box::new(|descr| unsafe {
        match RUNNING.take() {
            Some(running) => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .append(true)
                    .create(true)
                    .open("test262_result")
                    .unwrap();
                file.write_all(
                    &format!("Test failed: {}, panic info: {}\n", running, descr).as_bytes(),
                )
                .unwrap();
            }
            _ => (),
        }
    }));
    let test262_path = "test262";
    let harness = Harness::new(test262_path).expect("failed to initialize harness");
    let mut skipped = 0;
    let mut passed = 0;
    let mut failed = 0;
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        for test in harness {
            match test {
                Ok(ref test) => {
                    unsafe {
                        RUNNING = Some(test.path.to_str().unwrap().to_string().clone());
                    }
                    if test.desc.flags.contains(&Flag::Module) {
                        continue;
                    }
                    let mut vm = VirtualMachine::new(Options::default());

                    let file = match std::fs::read(&test.path) {
                        Err(e) => {
                            eprintln!(
                                "Error happened while openning test file,skipping... (Error: {})",
                                e
                            );
                            skipped += 1;
                            continue;
                        }
                        Ok(file) => String::from_utf8(file).unwrap(),
                    };
                    let mut code = String::new();

                    for include in test.desc.includes.iter() {
                        code.push_str(
                            &String::from_utf8(
                                std::fs::read(&format!("test262/harness/{}", include)).unwrap(),
                            )
                            .unwrap(),
                        );
                    }
                    code.push_str(&file);

                    let force_strict = test.desc.flags.contains(&Flag::OnlyStrict);
                    let not_strict = test.desc.flags.contains(&Flag::NoStrict);
                    let _raw = test.desc.flags.contains(&Flag::Raw);

                    let fun = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        match vm.compile(force_strict && !not_strict && !_raw, &code, "test") {
                            Ok(val) => Some(val.root(&mut vm)),
                            Err(_) => match &test.desc.negative {
                                Some(neg) => match neg.phase {
                                    Phase::Early | Phase::Parse => {
                                        passed += 1;
                                        None
                                    }
                                    _ => {
                                        failed += 1;
                                        None
                                    }
                                },
                                _ => {
                                    failed += 1;
                                    None
                                }
                            },
                        }
                    }));
                    let panic_ = std::panic::catch_unwind(AssertUnwindSafe(|| match fun {
                        Ok(Some(mut val)) => {
                            let args = Arguments::new(&mut vm, JsValue::undefined(), 0);
                            let mut args = Handle::new(&mut vm.space(), args);
                            match val.as_function_mut().call(&mut vm, &mut args) {
                                Ok(_) => (),
                                Err(_) => match &test.desc.negative {
                                    Some(neg) => match neg.phase {
                                        Phase::Runtime => {
                                            passed += 1;
                                        }
                                        _ => failed += 1,
                                    },
                                    _ => failed += 1,
                                },
                            }
                        }
                        Err(_) => {
                            failed += 1;
                        }
                        _ => {}
                    }));
                    match panic_ {
                        Ok(_) => (),
                        Err(_e) => match &test.desc.negative {
                            Some(neg) => match neg.phase {
                                Phase::Runtime => {
                                    passed += 1;
                                }
                                _ => failed += 1,
                            },
                            _ => failed += 1,
                        },
                    }

                    VirtualMachineRef::dispose(vm);
                }
                Err(e) => {
                    println!(
                        "Error happened while openning test,skipping... (Error: {})",
                        e
                    );
                    skipped += 1;
                }
            }
        }
    }));
    println!(
        "test262 results: \n Passed: {} \n Skipped: {} \n Failed: {} \n ",
        passed, skipped, failed
    );
}
