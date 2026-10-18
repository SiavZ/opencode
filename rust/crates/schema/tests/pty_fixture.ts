import { Pty } from "../../../packages/schema/src/pty.ts";

const info: Pty.Info = {
  id: "pty_1",
  title: "t",
  command: "c",
  args: ["a"],
  cwd: "/",
  status: "running",
  pid: 123,
  exitCode: 0,
};

const infoNoExit: Pty.Info = {
  id: "pty_2",
  title: "t",
  command: "c",
  args: [],
  cwd: "/",
  status: "exited",
  pid: 456,
};

const create: Pty.CreateInput = {
  command: "c",
  args: ["a"],
  cwd: "/",
  title: "t",
  env: { A: "B" },
};

const update: Pty.UpdateInput = {
  title: "t",
  size: { rows: 1, cols: 2 },
};

const connectToken: Pty.ConnectToken = {
  ticket: "tok",
  expires_in: 100,
};

console.log("INFO:", JSON.stringify(info));
console.log("INFO_NO_EXIT:", JSON.stringify(infoNoExit));
console.log("CREATE:", JSON.stringify(create));
console.log("UPDATE:", JSON.stringify(update));
console.log("CONNECT_TOKEN:", JSON.stringify(connectToken));
