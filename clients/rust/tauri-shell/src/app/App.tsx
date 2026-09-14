import { useState, useMemo, useEffect } from "react"
import { Sidebar } from "@/features/shell/Sidebar"
import { ComputersPage } from "@/features/library/ComputersPage"
import { SessionView } from "@/features/session/SessionView"
import {
  createConnection,
  type ConnectionInstance,
  type ConnectionSnapshot,
} from "@/lib/connection"
import {
  isNativeAvailable,
  invokeCommand,
  agentReleaseAll,
  type MahoCommand,
} from "@/lib/ipc"

export interface AppProps {
  connection?: ConnectionInstance;
}

export function App(props: AppProps = {}) {
  const propConnection = props.connection
  const [activeView, setActiveView] = useState<"computers" | "favorites">("computers")
  const native = isNativeAvailable()

  const connection: ConnectionInstance = useMemo(() => {
    return (
      propConnection ??
      createConnection({
        invoke: (cmd: string, args?: unknown) =>
          invokeCommand(cmd as MahoCommand, args as Record<string, unknown> | undefined),
        nativeAvailable: native,
        releaseInputs: async () => {
          try {
            await agentReleaseAll()
          } catch {
            // Release failure is non-fatal during teardown
          }
        },
      })
    )
  }, [propConnection, native])

  const [snapshot, setSnapshot] = useState<ConnectionSnapshot>(() =>
    connection.snapshot()
  )

  useEffect(() => {
    setSnapshot(connection.snapshot())
    return connection.subscribe(setSnapshot)
  }, [connection])

  const isSessionActive =
    snapshot.phase === "connecting" ||
    snapshot.phase === "waiting-video" ||
    snapshot.phase === "streaming"

  if (isSessionActive) {
    return (
      <div className="fixed inset-0 w-screen h-screen overflow-hidden bg-black select-none">
        <SessionView connection={connection} snapshot={snapshot} />
      </div>
    )
  }

  return (
    <div
      className="flex h-screen w-screen overflow-hidden bg-background text-foreground"
      data-app-shell
    >
      <Sidebar active={activeView} onSelect={setActiveView} />
      <main
        className="flex-1 min-w-0 h-full overflow-y-auto p-8 max-[1040px]:p-6 max-[720px]:p-4"
        id="main-view"
        data-ui-scope
      >
        <ComputersPage view={activeView} connection={connection} />
      </main>
    </div>
  )
}

export default App
