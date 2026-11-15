import React from "react";
import ReactDOM from "react-dom/client";
import WebApp from "./WebApp";
import "./web.css";

const root = document.getElementById("root");
if (!root) throw new Error("缺少页面根节点");
ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <WebApp />
  </React.StrictMode>,
);
