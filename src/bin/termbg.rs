fn main() {
    let timeout = std::time::Duration::from_millis(100);

    println!("Check terminal background color");
    let term = termbg_with_async_stdin::terminal();
    let latency = termbg_with_async_stdin::latency(std::time::Duration::from_millis(1000));
    let rgb = termbg_with_async_stdin::rgb(timeout);
    let theme = termbg_with_async_stdin::theme(timeout);

    println!("  Term : {:?}", term);

    match latency {
        Ok(latency) => {
            println!("  Latency: {:?}", latency);
        }
        Err(e) => {
            println!("  Latency: detection failed {:?}", e);
        }
    }

    match rgb {
        Ok(rgb) => {
            println!("  Color: R={:x}, G={:x}, B={:x}", rgb.r, rgb.g, rgb.b);
        }
        Err(e) => {
            println!("  Color: detection failed {:?}", e);
        }
    }

    match theme {
        Ok(theme) => {
            println!("  Theme: {:?}", theme);
        }
        Err(e) => {
            println!("  Theme: detection failed {:?}", e);
        }
    }
}
