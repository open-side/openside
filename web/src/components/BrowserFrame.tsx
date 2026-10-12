export function BrowserFrame({ data }: { data: string }) {
  return <img src={`data:image/jpeg;base64,${data}`} alt="Remote browser tab" draggable={false} />;
}
