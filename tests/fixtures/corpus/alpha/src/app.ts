export function load() {
  try {
    execute();
  } catch (error) {
    report();
  }
}
